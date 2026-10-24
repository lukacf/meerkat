#![allow(
    unused_imports,
    clippy::clone_on_copy,
    clippy::implicit_clone,
    clippy::redundant_clone
)]

mod agent_input;
mod agent_tools;
mod child_mcp_servers;
mod child_tool_bundles;
mod child_tool_policy;
pub mod council_relink;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod detached_completion_sink;
#[cfg(target_arch = "wasm32")]
#[path = "detached_completion_sink_wasm.rs"]
pub(crate) mod detached_completion_sink;
pub mod detached_delivery;
pub mod fork_relink;
#[cfg(all(feature = "openai-live", not(target_arch = "wasm32")))]
pub mod live_delegation;
mod public_definition;
mod public_mcp;
pub mod run_accounting;
#[cfg(not(target_arch = "wasm32"))]
mod schedule_host;
mod state_tasks;
mod surface;
pub mod temporary_council;
mod workgraph_flow;
pub use agent_tools::{
    AgentMobToolSurface, AgentMobToolSurfaceFactory, archive_session_with_mob_cleanup,
};
pub use child_mcp_servers::{ChildMcpServerRegistrationError, ChildMcpServers};
pub use child_tool_bundles::{ChildToolBundleAvailability, ChildToolBundles};
pub use child_tool_policy::ChildToolPolicyRefused;
#[cfg(not(target_arch = "wasm32"))]
pub use detached_completion_sink::{HostJobSource, HostJobSourceError};
pub use detached_delivery::{
    DetachedCompletionDelivered, DetachedCompletionError, DetachedDeliveryUnavailable,
    DetachedOwnerError, DetachedOwnerHost, detached_completion_notice,
};
pub use public_definition::{decode_public_mob_definition, decode_public_profile};
pub use public_mcp::{
    handle_public_tools_call, public_tool_names, public_tools_list,
    public_tools_list_without_workgraph, wrap_public_tool_payload,
};
#[cfg(not(target_arch = "wasm32"))]
pub use schedule_host::MobMcpScheduleHost;
pub use surface::wire_mob_tools;
pub use temporary_council::{
    MergeBackPolicy, TEMPORARY_COUNCIL_CLAIM_LEASE, TEMPORARY_COUNCIL_CLEANUP_BUDGET,
    TemporaryCouncilBounds, TemporaryCouncilCoordinator, TemporaryCouncilDeadline,
    TemporaryCouncilError, TemporaryCouncilHeldRecord, TemporaryCouncilOutcome,
    TemporaryCouncilParticipantSpec, TemporaryCouncilRecoveryReport, TemporaryCouncilRecoverySweep,
    TemporaryCouncilRequest, TemporaryCouncilStructuredContract,
};
pub use workgraph_flow::{
    AbandonUncertainWorkGraphFlowRequest, LaunchWorkGraphFlowRequest, WorkGraphFlowAbandonResult,
    WorkGraphFlowBridgeError, WorkGraphFlowLaunchResult, WorkGraphFlowReconcileResult,
};

#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use meerkat_core::time_compat::wasm as time;
    pub use meerkat_core::tokio::spawn;
    pub use meerkat_core::tokio::task;
    pub use tokio_with_wasm::alias::*;
}

use async_trait::async_trait;

/// The continuation producer namespace a keyed member send commits under.
/// Core owns the `meerkat.` host namespaces, so no host source shares it.
pub const MEMBER_SEND_CONTINUATION_NAMESPACE: &str = "meerkat.member_send";

/// The prefix a caller's member-send key carries in the continuation key
/// ledger, so it never meets another producer's key for the same member.
pub const MEMBER_SEND_CONTINUATION_KEY_PREFIX: &str = "member_send:";

use meerkat_client::LlmClient;
use meerkat_contracts::{
    MobDefinitionInput, MobLifecycleParams, MobSpawnManyResultEntry, WireContentInput,
    WireMemberRef, WireMobLifecycleAction, WireMobLifecycleStatus, WireMobRespawnOutcome,
    WireMobWireAction,
};
use meerkat_core::AppendSystemContextStatus;
use meerkat_core::ScopedAgentEvent;
use meerkat_core::agent::{
    AgentToolDispatcher, BindOutcome, CommsCapabilityError, CommsRuntime as CoreCommsRuntime,
    DispatcherCapabilities, OpsLifecycleBindError,
};
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationResult,
    GeneratedCommsTrustAuthoritySourceKind, PeerId, PeerName, SendError, SendReceipt,
    TrustedPeerDescriptor,
};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionId, PeerInputCandidate};
use meerkat_core::ops::{AsyncOpRef, OperationId, OperationResult};
use meerkat_core::ops_lifecycle::{OperationKind, OperationSpec, OpsLifecycleRegistry};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionHistoryPage, SessionHistoryQuery, SessionInfo,
    SessionQuery, SessionService, SessionServiceCommsExt, SessionServiceControlExt,
    SessionServiceHistoryExt, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::time_compat::{Instant, SystemTime};
use meerkat_core::types::{
    ContentInput, HandlingMode, RenderMetadata, RunResult, SessionId, ToolCallView, ToolDef,
    ToolProvenance, ToolResult, ToolSourceKind, Usage,
};
use meerkat_core::{AgentEvent, EventEnvelope, EventStream, Provider, StreamError};
use meerkat_mob::definition::SkillSource;
use meerkat_mob::machines::mob_machine::ControlScope;
use meerkat_mob::store::{
    ForkedParticipantStore, InMemoryForkedParticipantStore, InMemoryTemporaryCouncilStore,
    TemporaryCouncilStore,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_mob::store::{SqliteForkedParticipantStore, SqliteTemporaryCouncilStore};
use meerkat_mob::{
    AgentIdentity, CommandAuthority, FlowId, MobBackendKind, MobBuilder, MobControlPrincipal,
    MobDefinition, MobError, MobHandle, MobId, MobRuntimeMode, MobSessionService, MobState,
    MobStorage, OperatorGrant, ProfileBinding, ProfileName, RunId, ScopeDenial, SpawnMemberSpec,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const LOCAL_FORKED_PARTICIPANT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::{Mutex, RwLock, mpsc};
#[cfg(target_arch = "wasm32")]
use tokio::sync::{Mutex, RwLock, mpsc};

/// What [`MobMcpState::close_admission`] or [`MobMcpState::stand_down`]
/// could prove before its deadline. Each operation documents what its
/// [`Self::Clean`] covers: a closed admission is not a drained state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobStateStandDown {
    /// Everything the called operation waits for settled: for
    /// `close_admission`, admission is closed and every managed mob answered
    /// its close; for `stand_down`, also every task the state owned finished
    /// and the state is sealed.
    Clean,
    /// Something the operation waits for did not settle by the deadline, or
    /// failed, or cannot be fenced on this target; each reason names it.
    Unsettled { reasons: Vec<String> },
}

impl MobStateStandDown {
    /// Whether the called operation's conditions all settled.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

/// A create parked after its native mob started and before the mob is
/// inserted into the managed set (test seam).
#[cfg(test)]
#[derive(Default)]
struct InsertHold {
    reached: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
}

/// Why a restored child mob is held. Private to this host: surfaces see only
/// its bounded token, through [`MobError::held_before_activation`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum ChildMobHoldReason {
    /// The owner session has no durable authority: neither committed nor
    /// archived.
    OwnerAbsent,
    /// The owner session's durable source could not be observed.
    OwnerUnobservable(String),
    /// The owner is archived, but the child's destroy was not admitted.
    TeardownBlocked(String),
    /// The child's own history could not be read to tell a recoverable
    /// destroy from a fresh one.
    HistoryUnreadable(String),
}

impl ChildMobHoldReason {
    fn token(&self) -> &'static str {
        match self {
            Self::OwnerAbsent => "owner_absent",
            Self::OwnerUnobservable(_) => "owner_unobservable",
            Self::TeardownBlocked(_) => "teardown_blocked",
            Self::HistoryUnreadable(_) => "history_unreadable",
        }
    }
}

impl std::fmt::Display for ChildMobHoldReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwnerAbsent => formatter.write_str("owner has no durable authority"),
            Self::OwnerUnobservable(detail) => {
                write!(formatter, "owner could not be observed: {detail}")
            }
            Self::TeardownBlocked(detail) => {
                write!(
                    formatter,
                    "owner is archived but the destroy was not admitted: {detail}"
                )
            }
            Self::HistoryUnreadable(detail) => {
                write!(formatter, "the mob's history could not be read: {detail}")
            }
        }
    }
}

/// A restored child mob held before activation: it has no actor, and its
/// store is kept exactly as recovered.
#[derive(Clone)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct HeldChildMob {
    mob_id: MobId,
    storage_path: PathBuf,
    owner: SessionId,
    reason: ChildMobHoldReason,
    /// The child's stored roster and recorded incarnation, kept by the first
    /// member lookup that reads them successfully and shared by every clone
    /// of this entry. A held child does not run and every move into or out
    /// of the held set takes the transfer lock, so it stays true while the
    /// entry exists; a re-hold makes a new entry. A failed read is not kept.
    roster_read: Arc<std::sync::OnceLock<HeldChildRosterRead>>,
}

/// One read of a held child's store for member lookups: every stored member
/// with a bridge session, by that session, and the incarnation its creation
/// record names (`None`: no record yet).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct HeldChildRosterRead {
    members: BTreeMap<SessionId, (AgentIdentity, meerkat_mob::Generation)>,
    incarnation: Option<meerkat_core::service::MobIncarnationId>,
}

/// Why a persisted mob was not restored: its resume failed at restore. The
/// mob has no actor, its store is kept exactly as found, and a lookup of its
/// id answers this cause, as the same typed error each time (`MobError` is
/// not `Clone`). One store that does not resume leaves the rest of the
/// restore alone.
#[derive(Clone)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum UnrestoredMobCause {
    IncarnationInconsistent(meerkat_mob::IncarnationInconsistency),
    IncarnationDestroyed(meerkat_core::service::MobIncarnationId),
    /// Any other refusal of the resume, as its message.
    Refused(String),
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl UnrestoredMobCause {
    fn of(error: &MobError) -> Self {
        match error {
            MobError::IncarnationInconsistent { inconsistency, .. } => {
                Self::IncarnationInconsistent(inconsistency.clone())
            }
            MobError::IncarnationDestroyed { incarnation, .. } => {
                Self::IncarnationDestroyed(*incarnation)
            }
            other => Self::Refused(other.to_string()),
        }
    }

    fn error(&self, mob_id: &MobId) -> MobError {
        match self {
            Self::IncarnationInconsistent(inconsistency) => MobError::IncarnationInconsistent {
                mob_id: mob_id.clone(),
                inconsistency: inconsistency.clone(),
            },
            Self::IncarnationDestroyed(incarnation) => MobError::IncarnationDestroyed {
                mob_id: mob_id.clone(),
                incarnation: *incarnation,
            },
            Self::Refused(reason) => {
                MobError::Internal(format!("mob {mob_id} was not restored: {reason}"))
            }
        }
    }
}

/// A persisted mob whose resume failed at restore: the id its store names,
/// and why.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct UnrestoredMob {
    mob_id: MobId,
    cause: UnrestoredMobCause,
}

/// A parked child-teardown preflight (test seam).
#[cfg(test)]
#[derive(Default)]
struct TeardownPreflightHold {
    reached: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
}

/// What restore does with one persisted mob, decided before any actor exists.
#[cfg(not(target_arch = "wasm32"))]
enum ChildMobRestoreGate {
    Resume,
    Teardown { mob_id: MobId, owner: SessionId },
    Hold(HeldChildMob),
}

/// Why an entry joins the managed set: only a host insert sets
/// `MobMcpState::handles_inserted` (see `MobMcpState::managed_mob_entry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum ManagedInsertOrigin {
    /// A create this state ran.
    OwnCreate,
    /// This state's restore from its persistent root.
    RootRestore,
    /// A handle a host built and inserted
    /// (`MobMcpState::try_mob_insert_handle`).
    HostInsert,
}

#[derive(Clone)]
struct ManagedMob {
    handle: MobHandle,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "read only by the native persistent-storage paths")
    )]
    storage_path: Option<PathBuf>,
}

/// The caller a destroy was admitted for. Only an equal caller collects the
/// destroy's report again or continues the storage cleanup it still owes;
/// the console and an agent surface never continue each other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MobDestroyCaller {
    /// This state's console principal, fixed at construction: the public,
    /// RPC and council entries and the state's own cleanup paths. It means
    /// nothing outside the state that recorded it.
    Console,
    /// An agent mob tool surface, by the identity its gate admitted: its
    /// control principal, the bridge session it serves, and the principal
    /// token of the mob operator authority it presented.
    Agent {
        control_principal: MobControlPrincipal,
        owner_bridge_session_id: SessionId,
        principal_token: meerkat_core::service::OpaquePrincipalToken,
    },
}

/// A mob this state destroyed: its members never serve again. The record
/// keeps the canonical report of the native destroy and the caller it was
/// admitted for, with the storage cleanup the destroy still owes.
#[derive(Debug)]
pub(crate) struct RetiredMob {
    caller: MobDestroyCaller,
    /// The incarnation of the destroyed mob: continuing its destroy needs
    /// manage scope over exactly this incarnation.
    pub(crate) incarnation: meerkat_core::service::MobIncarnationId,
    report: meerkat_mob::MobDestroyReport,
    /// The destroyed mob's database, when it had one.
    storage: Option<RetiredMobStorage>,
}

#[derive(Debug)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct RetiredMobStorage {
    path: PathBuf,
    /// `None` once nothing is owed. Held across a cleanup attempt, so
    /// attempts run one at a time.
    owed: Mutex<Option<OwedStorageCleanup>>,
}

/// The storage cleanup a natively completed destroy still owes, with what
/// was taken before the destroy started.
#[derive(Debug)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct OwedStorageCleanup {
    /// What the database path held.
    before: StorageFileSeen,
    /// The mob's actor task: the stores it owns drop only once it ends.
    actor_task: meerkat_mob::MobActorTaskWatch,
}

/// What a storage path held when it was looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum StorageFileSeen {
    Absent,
    /// A file, with its device and inode where the platform reports them.
    Present(Option<(u64, u64)>),
}

impl RetiredMob {
    /// Whether storage cleanup is still owed; an attempt that is running
    /// counts as owed.
    fn cleanup_owed(&self) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|storage| match storage.owed.try_lock() {
                Ok(owed) => owed.is_some(),
                Err(_) => true,
            })
    }

    /// A copy of the canonical report that names the outstanding cleanup.
    /// The record keeps the canonical report itself, so a later complete
    /// answer carries no stale cleanup error.
    fn report_with_outstanding_cleanup(&self, outstanding: &str) -> meerkat_mob::MobDestroyReport {
        let mut report = self.report.clone();
        report
            .errors
            .push(format!("storage cleanup outstanding: {outstanding}"));
        report
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl StorageFileSeen {
    /// Look at `path` without following a final symlink: removal unlinks
    /// that entry.
    async fn look(path: &Path) -> std::io::Result<Self> {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => Ok(Self::Present(Self::identity(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn identity(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt as _;
        Some((metadata.dev(), metadata.ino()))
    }

    /// No device and inode here: a file present before and after the
    /// destroy is taken to be the same one.
    #[cfg(not(unix))]
    fn identity(_metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl OwedStorageCleanup {
    /// What a destroy will owe the mob's database at `path`, taken before
    /// the destroy starts, with the mob's actor task.
    async fn before_destroy(
        path: &Path,
        actor_task: meerkat_mob::MobActorTaskWatch,
    ) -> Result<Self, MobError> {
        let before = StorageFileSeen::look(path).await.map_err(|error| {
            MobError::Internal(format!(
                "mob storage '{}' could not be looked at before destroy: {error}",
                path.display()
            ))
        })?;
        Ok(Self { before, actor_task })
    }
}

/// A mob a create built: its id and the incarnation its create minted, as
/// the witness its handle hands out.
#[derive(Debug, Clone)]
pub(crate) struct CreatedMob {
    pub(crate) mob_id: MobId,
    pub(crate) incarnation: meerkat_core::service::LiveMobIncarnation,
}

/// A hosted roster that names a session as one of its members' sessions
/// ([`MobMcpState::session_member_matches`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionMemberMatch {
    /// The mob whose roster names the session.
    pub mob_id: MobId,
    /// The incarnation that mob runs at, or was created at for a held child
    /// mob.
    pub incarnation: meerkat_core::service::MobIncarnationId,
    /// The member the session serves.
    pub identity: AgentIdentity,
    /// That member's generation.
    pub generation: meerkat_mob::Generation,
}

/// Who makes a create for a caller session, which decides whether the
/// mob's creation record names that session as the creator whose manage
/// grant the agent mob tools re-derive after a restart
/// ([`MobMcpState::session_is_recorded_creator`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCreateCaller {
    /// A host creates the mob for the session
    /// ([`MobMcpState::mob_create_definition_with_owner_bridge_session`]):
    /// the session owns the mob but holds no grant from the create, so the
    /// record names it `Session` or `Member`.
    Host,
    /// The session creates the mob with its own create capability (the
    /// agent mob tools' `mob_create`), whose answer grants it manage over
    /// the mob: the record names it `OperatorSession` or `OperatorMember`.
    Session,
}

/// The provenance a create by `session_id` records, from the complete
/// lookup of the hosted rosters that name that session
/// ([`MobMcpState::session_member_matches`]): the member when exactly one
/// roster names it, the bare session when none does, and `Ambiguous` when
/// several do, without picking one. `caller` decides whether the member or
/// session is recorded as the creator of its own create.
fn session_creation_provenance(
    session_id: &SessionId,
    matches: Vec<SessionMemberMatch>,
    caller: SessionCreateCaller,
) -> meerkat_mob::MobProvenance {
    match (<[SessionMemberMatch; 1]>::try_from(matches), caller) {
        (Ok([found]), SessionCreateCaller::Host) => meerkat_mob::MobProvenance::Member {
            parent_mob_id: found.mob_id,
            parent_incarnation: found.incarnation,
            creator_identity: found.identity,
            creator_generation: found.generation,
            owner_session: session_id.clone(),
        },
        (Ok([found]), SessionCreateCaller::Session) => meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id: found.mob_id,
            parent_incarnation: found.incarnation,
            creator_identity: found.identity,
            creator_generation: found.generation,
            owner_session: session_id.clone(),
        },
        (Err(matches), SessionCreateCaller::Host) if matches.is_empty() => {
            meerkat_mob::MobProvenance::Session {
                session_id: session_id.clone(),
            }
        }
        (Err(matches), SessionCreateCaller::Session) if matches.is_empty() => {
            meerkat_mob::MobProvenance::OperatorSession {
                session_id: session_id.clone(),
            }
        }
        (Err(matches), _) => meerkat_mob::MobProvenance::Ambiguous {
            matching_rosters: u32::try_from(matches.len()).unwrap_or(u32::MAX),
        },
    }
}

/// How a destroy request meets this state's record of the mobs it
/// destroyed.
pub(crate) enum RetiredDestroyLookup {
    /// No destroy under the id still owes its storage cleanup, and the id is
    /// managed or this state keeps nothing under it for this request: the
    /// request takes the ordinary path.
    Ordinary,
    /// This state destroyed the mob for another caller and still owes its
    /// storage cleanup, which only that caller continues.
    OtherCaller,
    /// This state destroyed the mob for this caller, and still owes its
    /// storage cleanup or manages no mob under the id again.
    Caller(Arc<RetiredMob>),
}

/// An admitted destroy with everything it needs to finish without the
/// caller that started it.
struct AdmittedDestroy {
    mob_id: MobId,
    caller: MobDestroyCaller,
    /// The console-bound handle the native destroy rides.
    bound: MobHandle,
    /// The destroyed mob instance's actor task. It tells that instance from
    /// another one registered under the same id meanwhile.
    actor_task: meerkat_mob::MobActorTaskWatch,
    storage: Option<RetiredMobStorage>,
    mobs: Arc<RwLock<BTreeMap<MobId, ManagedMob>>>,
    retired_mobs: Arc<std::sync::Mutex<BTreeMap<MobId, Arc<RetiredMob>>>>,
    mob_set_epoch: tokio::sync::watch::Sender<u64>,
    /// The host's release hook, run once the native destroy completed and
    /// the mob left the managed set.
    after_destroy: Option<MobAfterDestroy>,
}

impl AdmittedDestroy {
    /// Run the native destroy, record its canonical report as it replies,
    /// before any other await, then settle the storage cleanup the destroy
    /// owes. Run as a task the state owns, a caller cancelled at any point
    /// loses neither the report nor that cleanup: the same caller collects
    /// or continues them. A record under the id that still owes its storage
    /// cleanup is never replaced: this destroy then answers from its own
    /// record without keeping it.
    async fn run(self) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        let Self {
            mob_id,
            caller,
            bound,
            actor_task,
            storage,
            mobs,
            retired_mobs,
            mob_set_epoch,
            after_destroy,
        } = self;
        let incarnation = bound.incarnation();
        let destroy_result = bound.destroy().await;
        // The authority-bound clone retains the SQLite event bus. Release it
        // before the storage cleanup so this surface owns nothing of the
        // observer that cleanup drains.
        drop(bound);
        let report = match destroy_result {
            Ok(report) => report,
            Err(meerkat_mob::MobDestroyError::Incomplete { report }) => {
                return Err(MobMcpDestroyError::Incomplete { report });
            }
            Err(meerkat_mob::MobDestroyError::Mob(error)) => {
                return Err(MobMcpDestroyError::Mob(error));
            }
            Err(other) => {
                // MobDestroyError is #[non_exhaustive]; future variants we
                // haven't coded for fall through to a generic internal
                // error so the caller still gets a readable message.
                return Err(MobMcpDestroyError::Mob(MobError::Internal(format!(
                    "mob destroy failed: {other}"
                ))));
            }
        };
        let retired = Arc::new(RetiredMob {
            caller,
            incarnation,
            report,
            storage,
        });
        {
            let mut records = retired_mobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Replacing a record whose storage cleanup is still owed would
            // lose that cleanup.
            if !records
                .get(&mob_id)
                .is_some_and(|recorded| recorded.cleanup_owed())
            {
                records.insert(mob_id.clone(), Arc::clone(&retired));
            }
        }
        // Remove the managed entry only while it is still the destroyed mob:
        // a handle registered under the id while the destroy ran is another
        // mob, which stays managed (and keeps its storage cleanup owed).
        let removed = {
            let mut managed = mobs.write().await;
            if managed
                .get(&mob_id)
                .is_some_and(|entry| actor_task.observes(&entry.handle))
            {
                managed.remove(&mob_id)
            } else {
                None
            }
        };
        if removed.is_some() {
            mob_set_epoch.send_modify(|epoch| *epoch += 1);
        }
        drop(removed);
        // Destroyed natively, recorded and no longer managed: the host
        // releases what it still holds of the mob before the storage cleanup
        // drains its database observer. Running it only now, and containing
        // a panic in it, keeps a failing hook from holding the id managed.
        run_after_destroy(after_destroy.as_ref(), &mob_id);
        MobMcpState::settle_retired_destroy(&mobs, &mob_id, &retired, after_destroy.as_ref()).await
    }
}

/// How many times a destroy's storage cleanup retries an observer drain
/// that other owner handles refused, within one cleanup attempt.
#[cfg(not(target_arch = "wasm32"))]
const OBSERVER_DRAIN_RETRIES: usize = 10;

/// The wait before each retried observer drain: with
/// [`OBSERVER_DRAIN_RETRIES`], a holder gets about a second to let go.
#[cfg(not(target_arch = "wasm32"))]
const OBSERVER_DRAIN_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Run the host's after-destroy hook for `mob_id`, if one is bound. A panic
/// in the hook is caught and logged, and the destroy goes on as if the hook
/// had returned: what the host failed to release only keeps the storage
/// cleanup outstanding, as a held handle does.
fn run_after_destroy(hook: Option<&MobAfterDestroy>, mob_id: &MobId) {
    let Some(hook) = hook else {
        return;
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(mob_id))).is_err() {
        tracing::error!(
            mob_id = %mob_id,
            "the host's after-destroy hook panicked; the destroy continues"
        );
    }
}

/// Structured reports a lifecycle action returns: `destroy` its cleanup
/// report, `stop` its per-member report (#1500).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct MobLifecycleReports {
    pub destroy_report: Option<meerkat_mob::MobDestroyReport>,
    pub stop_report: Option<meerkat_mob::MobStopReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum MobMcpDestroyError {
    #[error("mob destroy incomplete: {}", destroy_report_summary(.report))]
    Incomplete {
        report: meerkat_mob::MobDestroyReport,
    },

    #[error(transparent)]
    Mob(#[from] MobError),
}

/// Error returned by the identity-addressed mob system-context surface.
///
/// Authorization belongs to the mob control plane, while the actual append
/// belongs to the member session service.  Keep both typed so a denied mob
/// principal retains the shared `ScopeDenied` wire detail instead of being
/// collapsed into a session-control string.
#[derive(Debug, thiserror::Error)]
pub enum MobAppendSystemContextError {
    #[error(transparent)]
    Mob(#[from] MobError),

    #[error(transparent)]
    Session(#[from] SessionControlError),
}

impl MobAppendSystemContextError {
    pub fn wire_detail(&self) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
        match self {
            Self::Mob(error) => error.wire_detail(),
            Self::Session(_) => None,
        }
    }

    pub fn wire_error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        match self {
            Self::Mob(error) => error.wire_error_code(),
            Self::Session(_) => None,
        }
    }
}

impl MobMcpDestroyError {
    pub fn incomplete_message(report: &meerkat_mob::MobDestroyReport) -> String {
        format!("mob destroy incomplete: {}", destroy_report_summary(report))
    }

    pub fn incomplete_error_data(report: &meerkat_mob::MobDestroyReport) -> serde_json::Value {
        json!(meerkat_contracts::WireMobDestroyIncompleteData::new(
            report.into()
        ))
    }

    pub fn error_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::Incomplete { report } => Some(Self::incomplete_error_data(report)),
            Self::Mob(_) => None,
        }
    }

    /// Delegating [`MobError::wire_detail`] (§17.4, ADJ-P7-4): `Mob(inner)`
    /// carries the inner console projection; the `Incomplete` report keeps
    /// its dedicated `mob_destroy_incomplete` data envelope.
    pub fn wire_detail(&self) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
        match self {
            Self::Incomplete { .. } => None,
            Self::Mob(inner) => inner.wire_detail(),
        }
    }

    pub fn wire_error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        match self {
            Self::Incomplete { .. } => None,
            Self::Mob(inner) => inner.wire_error_code(),
        }
    }

    fn into_mob_error(self) -> MobError {
        match self {
            Self::Incomplete { report } => MobError::Internal(Self::incomplete_message(&report)),
            Self::Mob(error) => error,
        }
    }

    pub fn into_session_error(self, context: &str) -> SessionError {
        match self {
            Self::Incomplete { report } => SessionError::FailedWithData {
                message: format!("{context}: {}", Self::incomplete_message(&report)),
                data: Self::incomplete_error_data(&report),
            },
            Self::Mob(error) => SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!("{context}: {error}")),
            ),
        }
    }
}

fn destroy_report_summary(report: &meerkat_mob::MobDestroyReport) -> String {
    if !report.errors.is_empty() {
        return report.errors.join("; ");
    }
    if report.remote_cleanup_deadline_exceeded {
        return "remote cleanup deadline exceeded".to_string();
    }
    if !report.orphaned_remote_members.is_empty() {
        return format!(
            "orphaned remote members: {}",
            report
                .orphaned_remote_members
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    "destroy cleanup did not complete".to_string()
}

type DefaultLlmClientProvider = Arc<dyn Fn() -> Option<Arc<dyn LlmClient>> + Send + Sync + 'static>;

/// One mob's member external tools, as a [`MemberExternalToolsFactory`]
/// builds them.
pub struct MemberExternalTools {
    /// The external tools the mob's members build with.
    pub provider: meerkat_mob::ExternalToolsProvider,
    /// Runs once, with the mob's id, when the mob built with `provider`
    /// enters this state's managed set, before any other caller can find it
    /// there. A build whose mob never enters it (a failed create, a create
    /// that lost to a duplicate, a refused restore) never runs it.
    pub on_managed: Option<MemberExternalToolsOnManaged>,
}

/// See [`MemberExternalTools::on_managed`].
pub type MemberExternalToolsOnManaged = Box<dyn FnOnce(&MobId) + Send>;

/// A callback route claim refused for its claim token
/// ([`MobMcpState::mob_claim_member_external_tools`]), before the claim ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallbackRouteClaimTokenRefused {
    /// The presented token is not the mob's claim token.
    #[error("the claim token is not the mob's")]
    Mismatch,
    /// The mob keeps no claim token (it was created other than over RPC, or
    /// before 0.9.3, and the host minted none), so no token claims its
    /// callback route.
    #[error("the mob has no claim token, so no connection can claim its callback route")]
    NotClaimable,
}

/// Builds the default external tools of one mob's members from the mob's id
/// (see [`MobMcpState::with_member_external_tools_factory`]).
pub type MemberExternalToolsFactory =
    Arc<dyn Fn(&MobId) -> Option<MemberExternalTools> + Send + Sync>;

/// The member external tools one build uses, and the hook that runs once
/// the mob it builds is managed.
type MemberExternalToolsBuild = (
    Option<meerkat_mob::ExternalToolsProvider>,
    Option<MemberExternalToolsOnManaged>,
);

#[doc(hidden)]
#[derive(Default)]
pub struct InMemoryArchiveFailureControl {
    failures: RwLock<HashMap<SessionId, String>>,
}

impl InMemoryArchiveFailureControl {
    pub async fn fail_archive(&self, id: SessionId, reason: impl Into<String>) {
        self.failures.write().await.insert(id, reason.into());
    }

    pub async fn clear_archive_failure(&self, id: &SessionId) {
        self.failures.write().await.remove(id);
    }

    async fn failure_for(&self, id: &SessionId) -> Option<String> {
        self.failures.read().await.get(id).cloned()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct KickoffMemberSnapshot {
    pub agent_identity: AgentIdentity,
    #[serde(flatten)]
    pub snapshot: meerkat_mob::MobMemberSnapshot,
}

/// Domain outcome for the RPC ingress composite.  Wire assembly remains in
/// the RPC crate, while admission, cursor capture, ensure, and delivery stay
/// inside one state-owned operation.
#[derive(Debug)]
pub struct MobIngressInteractionOutcome {
    /// Whether the declarative member step spawned or retained the target.
    pub ensure_outcome: meerkat_mob::runtime::EnsureMemberOutcome,
    /// Canonical member-delivery receipt.
    pub delivery: meerkat_mob::MemberDeliveryReceipt,
    /// Structural-event cursor observed before ensure+delivery.
    pub events_after_cursor: u64,
    /// Structural-event cursor observed after delivery admission.
    pub latest_event_cursor: u64,
}

fn persisted_mob_binding(
    view: &meerkat_core::PersistedSessionMetadataView,
) -> Option<meerkat_mob::MobId> {
    // Read the typed durable identity directly off the metadata view. Old
    // persisted rows written before `mob_member_binding` existed decode as
    // `None` and are simply not owned (back-read safe) — they re-acquire a
    // typed binding on the next build/resume. No comms_name string-split, no
    // `mob:{id}` realm format-string, no magic-key peer_meta cross-check.
    let binding = view.mob_member_binding()?;
    Some(meerkat_mob::MobId::from(binding.mob_id.as_str()))
}

/// Typed provenance of the shared realm-scoped profile store.
///
/// Replaces the former `(Option<Arc<dyn RealmProfileStore>>, explicit: bool)`
/// pair: the variant itself encodes *why* the store has its current value, so
/// the persistent-root auto-upgrade decision matches a variant instead of
/// re-deriving intent from a parallel bool. Builder-local only; never
/// serialized, persisted, or crossing any wire/surface boundary.
enum RealmProfileStoreSelection {
    /// Default in-memory store installed by the constructor. Eligible for
    /// auto-upgrade to durable SQLite under a persistent storage root.
    DefaultInMemory(Arc<dyn meerkat_mob::RealmProfileStore>),
    /// Default store that has already been upgraded to durable SQLite under a
    /// persistent storage root. Must not be upgraded again.
    ///
    /// Only constructed on non-wasm targets (the SQLite persistent-root upgrade
    /// is `#[cfg(not(wasm32))]`); on wasm32 it is an unreachable arm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DefaultDurable(Arc<dyn meerkat_mob::RealmProfileStore>),
    /// Caller-supplied store (or explicit `None` to disable). Must never be
    /// overridden by the persistent-root auto-upgrade.
    CallerSupplied(Option<Arc<dyn meerkat_mob::RealmProfileStore>>),
}

impl RealmProfileStoreSelection {
    /// Resolved store, regardless of provenance.
    fn store(&self) -> Option<&Arc<dyn meerkat_mob::RealmProfileStore>> {
        match self {
            Self::DefaultInMemory(store) | Self::DefaultDurable(store) => Some(store),
            Self::CallerSupplied(opt) => opt.as_ref(),
        }
    }

    /// Whether this is the auto-upgradeable default in-memory store. Only
    /// consulted by the non-wasm persistent-root auto-upgrade.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn is_default(&self) -> bool {
        matches!(self, Self::DefaultInMemory(_))
    }
}

/// Typed provenance of the state-owned, realm-scoped capability custody.
///
/// Each variant always contains exactly one store. The state composition layer
/// replaces only the default in-memory store when an explicit persistent root
/// is installed; a host-injected store is never silently replaced.
enum ForkedParticipantStoreSelection {
    DefaultInMemory(Arc<dyn ForkedParticipantStore>),
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DefaultDurable(Arc<dyn ForkedParticipantStore>),
    CallerSupplied(Arc<dyn ForkedParticipantStore>),
}

impl ForkedParticipantStoreSelection {
    fn store(&self) -> &Arc<dyn ForkedParticipantStore> {
        match self {
            Self::DefaultInMemory(store)
            | Self::DefaultDurable(store)
            | Self::CallerSupplied(store) => store,
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn is_default(&self) -> bool {
        matches!(self, Self::DefaultInMemory(_))
    }
}

/// Typed provenance of the state-owned, realm-scoped temporary-council custody.
///
/// Mirrors [`ForkedParticipantStoreSelection`] exactly, including the rule that
/// only the default in-memory store is replaced when an explicit persistent
/// root is installed.
enum TemporaryCouncilStoreSelection {
    DefaultInMemory(Arc<dyn TemporaryCouncilStore>),
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    DefaultDurable(Arc<dyn TemporaryCouncilStore>),
    CallerSupplied(Arc<dyn TemporaryCouncilStore>),
}

impl TemporaryCouncilStoreSelection {
    fn store(&self) -> &Arc<dyn TemporaryCouncilStore> {
        match self {
            Self::DefaultInMemory(store)
            | Self::DefaultDurable(store)
            | Self::CallerSupplied(store) => store,
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn is_default(&self) -> bool {
        matches!(self, Self::DefaultInMemory(_))
    }
}

/// Why a mob state could not bind the host's continuation services.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BindContinuationsError {
    /// The state has no runtime to admit completions.
    #[error("the mob state has no runtime")]
    NoRuntime,
    /// The host's binding slot refused this state.
    #[error(transparent)]
    Binding(meerkat::ContinuationBindError),
}

/// In-memory MCP state for multiple mobs.
/// Whether a host can deliver a detached tool completion after the call
/// returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetachedCompletionDelivery {
    /// The host outlives the call: detached tools return a handle and their
    /// completion is delivered to the owner session later.
    Available,
    /// The host may exit when the turn ends: detached-capable tools block for
    /// their result instead.
    Unavailable,
}

/// Host release hook for the mobs a [`MobMcpState`] destroys: called with a
/// destroyed mob's id once its native destroy completed and it left the
/// managed set, before the storage cleanup that destroy owes. See
/// [`MobMcpState::set_after_destroy`].
pub type MobAfterDestroy = Arc<dyn Fn(&MobId) + Send + Sync>;

/// What a [`MobBeforeCreate`] hook is told about a mob a [`MobMcpState`] is
/// about to create. Every field comes from the create path, never from the
/// caller's input: a model's `mob_create` names the mob's id, nothing else
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MobCreateRequest {
    /// The id the mob would be created under.
    pub mob_id: MobId,
    /// The owner bridge session the mob would be created for: the calling
    /// agent's session for its `mob_create` and its implicit delegation
    /// mob, a session of its own for a temporary council. `None` only for a
    /// mob a host creates directly ([`MobMcpState::mob_create_definition`],
    /// [`MobMcpState::mob_create_definition_with_member_external_tools`],
    /// [`MobMcpState::mob_create_from_mobpack`]).
    pub owner_bridge_session_id: Option<SessionId>,
    /// Whether this is the owner session's implicit delegation mob, created
    /// by that session's own runtime path
    /// ([`MobMcpState::ensure_implicit_mob_for_model`]) under the id it
    /// derives from the session ([`MobDefinition::implicit`]). Never set for
    /// a mob a caller names, whatever its id.
    pub implicit_delegation_mob: bool,
}

/// Host admission hook for the mobs a [`MobMcpState`] creates: `Ok(())` lets
/// the create proceed, `Err(reason)` refuses it. See
/// [`MobMcpState::set_before_create`].
#[cfg(not(target_arch = "wasm32"))]
pub type MobBeforeCreate = Arc<
    dyn Fn(
            MobCreateRequest,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;
/// Host admission hook for the mobs a [`MobMcpState`] creates: `Ok(())` lets
/// the create proceed, `Err(reason)` refuses it. See
/// [`MobMcpState::set_before_create`].
#[cfg(target_arch = "wasm32")]
pub type MobBeforeCreate = Arc<
    dyn Fn(
            MobCreateRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>
        + Send
        + Sync,
>;

/// The `kind` in the structured data of a host's refusal of a mob create
/// ([`MobMcpState::set_before_create`], [`MobMcpState::is_create_refusal`]).
pub const MOB_CREATE_REFUSED_KIND: &str = "mob_create_refused";

/// The reason a create is refused with when the host's create hook panicked:
/// its answer is unknown, so the create does not proceed.
const BEFORE_CREATE_PANICKED: &str =
    "the host's create admission panicked, so the create is refused";

pub struct MobMcpState {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_llm_client_provider: Option<DefaultLlmClientProvider>,
    external_tools_provider: Option<meerkat_mob::ExternalToolsProvider>,
    /// Per-mob member external tools (see [`MemberExternalToolsFactory`]).
    member_external_tools_factory: Option<MemberExternalToolsFactory>,
    /// Host selection of member destinations, inherited by every child mob.
    member_runtime_registration_hook: Option<Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>>,
    before_activation: std::sync::OnceLock<meerkat_mob::MobBeforeActivation>,
    /// Host release hook for the mobs this state destroys; see
    /// [`MobMcpState::set_after_destroy`].
    after_destroy: std::sync::OnceLock<MobAfterDestroy>,
    /// Host admission hook for the mobs this state creates; see
    /// [`MobMcpState::set_before_create`].
    before_create: std::sync::OnceLock<MobBeforeCreate>,
    /// Host bundles; only the child-available ones reach child mob builders.
    child_tool_bundles: ChildToolBundles,
    additional_child_tool_bundles: std::sync::OnceLock<ChildToolBundles>,
    /// Public descriptors supplied at child creation, never protected configs.
    child_mcp_servers: std::sync::RwLock<ChildMcpServers>,
    /// Host consequence-policy registry, forwarded to every child builder.
    tool_consequence_policy_registry: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    /// The host's explicit application tool policy for child mob members.
    child_application_tool_policy: Option<meerkat_core::ApplicationToolPolicyBinding>,
    /// The host's spawn customizer for child mob members, run before the
    /// internal child policy customizer.
    child_spawn_member_customizer: Option<Arc<dyn meerkat_mob::SpawnMemberCustomizer>>,
    persistent_storage_root: Option<PathBuf>,
    /// Legacy infallible persistent-root construction records setup failure so
    /// every managed-mob operation fails closed rather than using ephemeral
    /// capability custody under a caller-requested durable root.
    persistent_storage_setup_error: Option<String>,
    mobs: Arc<RwLock<BTreeMap<MobId, ManagedMob>>>,
    /// Mobs this state destroyed: their members never serve again. Each
    /// record keeps the canonical report, the caller the destroy was
    /// admitted for, and the storage cleanup it still owes.
    retired_mobs: Arc<std::sync::Mutex<BTreeMap<MobId, Arc<RetiredMob>>>>,
    /// Restored child mobs held because their owner is unresolved: never
    /// resumed, so no actor or producer exists for them and nothing in their
    /// durable state is changed by the hold.
    held_child_mobs: RwLock<BTreeMap<MobId, HeldChildMob>>,
    /// Persisted mobs whose resume failed at the last restore pass, by store
    /// path, with why: never registered, their stores kept as found. Each
    /// pass rebuilds it.
    unrestored_mobs: RwLock<BTreeMap<PathBuf, UnrestoredMob>>,
    /// Serializes moving a held child out of the hold. Its held row stays
    /// until the child is registered as managed (or confirmed gone), so a
    /// cancelled or failed transfer leaves it held, never unaccounted for.
    #[cfg(not(target_arch = "wasm32"))]
    held_child_transfer: Mutex<()>,
    /// Test seam: when set, a child teardown parks right before its WorkGraph
    /// preflight, so a test can cancel the caller exactly there.
    #[cfg(test)]
    teardown_preflight_hold: std::sync::Mutex<Option<Arc<TeardownPreflightHold>>>,
    /// Set once the managed set holds every mob this host will ever have
    /// from before: persistent restore finished, or the host declared its
    /// own restore done ([`MobMcpState::declare_mob_set_restored`]). A mob
    /// outside the set is then gone for good.
    mob_set_restored: std::sync::atomic::AtomicBool,
    /// Set by the first handle a host inserts, under the managed set's write
    /// lock and before the handle is visible. Every managed-set entry is
    /// built by [`MobMcpState::managed_mob_entry`], which sets it for a host
    /// insert, and only [`MobMcpState::try_mob_insert_handle`] inserts as the
    /// host, so no insert skips it. Until it is set a state without a
    /// persistent root hosts only mobs it created in this process, so a
    /// lookup over its rosters is complete without a declared restore. It is
    /// never cleared.
    handles_inserted: std::sync::atomic::AtomicBool,
    /// Bumped whenever the managed-mob handle set gains or loses an entry.
    /// Interval-free observers (e.g. meta's agent-event stream reconcilers)
    /// await this instead of polling `mob_handles_snapshot` on a timer. The
    /// value is an opaque epoch, not a count; only "changed" is meaningful.
    mob_set_epoch: tokio::sync::watch::Sender<u64>,
    /// Per-session locks for single-flight implicit mob creation.
    implicit_mob_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    workgraph_flow_custodies: Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    workgraph_flow_reconcile_epoch: tokio::sync::watch::Sender<u64>,
    workgraph_flow_reconciler_started: std::sync::atomic::AtomicBool,
    restore_lock: Mutex<bool>,
    /// Shared realm-scoped profile store for cross-mob profile CRUD, together
    /// with its provenance (single source of truth for the persistent-root
    /// auto-upgrade decision).
    realm_profile_store_selection: RealmProfileStoreSelection,
    /// One source-owned capability custody store shared by every mob built by
    /// this state. It is realm/state scoped, never a process-global singleton.
    forked_participant_store_selection: ForkedParticipantStoreSelection,
    /// Realm-scoped temporary-council orchestration custody (issue #159).
    temporary_council_store_selection: TemporaryCouncilStoreSelection,
    /// Single-flight registry of owned council execution tasks, keyed by
    /// council id. A `std` mutex because the map is only ever touched without
    /// awaits, including from the task's drop guard.
    temporary_council_inflight:
        std::sync::Mutex<HashMap<String, temporary_council::InflightCouncil>>,
    /// Weak self-reference, installed the first time an `Arc<Self>` is
    /// available. Persistent restoration uses it to schedule the council
    /// recovery sweep automatically instead of requiring a caller to run
    /// maintenance by hand.
    self_weak: std::sync::OnceLock<std::sync::Weak<MobMcpState>>,
    /// Test seam: when set, a create parks after its native mob started and
    /// before the managed-set insertion.
    #[cfg(test)]
    create_insert_hold: std::sync::Mutex<Option<Arc<InsertHold>>>,
    /// Test seam: when set, a persisted resume parks after its native resume
    /// and before the managed-set registration.
    #[cfg(test)]
    resume_insert_hold: std::sync::Mutex<Option<Arc<InsertHold>>>,
    /// Test seam: the store at this path is refused at its resume as one
    /// whose creation record disagrees with its log.
    #[cfg(test)]
    refused_resume_store: std::sync::Mutex<Option<PathBuf>>,
    /// Test seam: when set, the next agent mob tool call (or keyed member
    /// send) parks once between its admission and its action (see
    /// [`Self::pause_agent_call`]).
    #[cfg(test)]
    agent_call_hold: std::sync::Mutex<Option<Arc<InsertHold>>>,
    /// Test seam: when set, a member lookup parks once between its first
    /// completeness check and its scan of the hosted rosters.
    #[cfg(test)]
    member_lookup_hold: std::sync::Mutex<Option<Arc<InsertHold>>>,
    /// Stable per-process coordinator identity. It is the council claim id, so
    /// a later in-process recovery sweep RENEWS the claim this process already
    /// holds instead of colliding with itself, while a genuinely different
    /// process presents a different identity and must go through the machine's
    /// busy/expiry-takeover path.
    coordinator_id: String,
    /// Wall-clock budget the coordinator gives ONE bounded cleanup pass before
    /// it publishes the already-sealed result with a pending receipt.
    temporary_council_cleanup_budget_ms: std::sync::atomic::AtomicU64,
    /// Offset applied to every clock read the temporary-council coordinator
    /// makes. Zero in production; the only way to move it is the explicit
    /// test-clock seam below, which no surface exposes.
    temporary_council_clock_offset_ms: std::sync::atomic::AtomicI64,
    /// Signals every change of the offset above, so a sweep waiting for a
    /// claim lease re-reads the clock instead of sleeping past a moved one.
    temporary_council_clock_changes: tokio::sync::watch::Sender<i64>,
    /// Bumped each time the post-restore council sweep finishes a pass, so
    /// tests can order work after a pass without polling.
    temporary_council_sweep_passes: tokio::sync::watch::Sender<u64>,
    /// Set once the automatic post-restore recovery sweep has been scheduled.
    /// Also what keeps the sweep from re-entering `ensure_restored`.
    temporary_council_recovery_scheduled: std::sync::atomic::AtomicBool,
    /// Every background task this state starts on its own, and whether it
    /// still admits work; see [`MobMcpState::stand_down`].
    pub(crate) state_tasks: state_tasks::StateTasks,
    /// Whether this host can deliver a detached tool completion to its
    /// owner session after the tool call returns. Declared by the host;
    /// never inferred.
    detached_completion_delivery: std::sync::atomic::AtomicBool,
    /// The host hook that makes a plain-session owner of a detached job live
    /// again (see [`DetachedOwnerHost`]). `None` when the host supplies none.
    detached_owner_host: std::sync::RwLock<Option<Arc<dyn DetachedOwnerHost>>>,
    /// Where detached outcomes are submitted, once the host bound its
    /// delivery inbox ([`MobMcpState::bind_continuations`]).
    detached_completions:
        std::sync::RwLock<Option<crate::detached_completion_sink::DetachedCompletionSink>>,
    /// This state's binding of the host's continuation services, held for
    /// the state's life (see [`MobMcpState::bind_continuations`]).
    #[cfg(not(target_arch = "wasm32"))]
    continuation_binding: std::sync::Mutex<Option<meerkat::ContinuationBinding>>,
    /// The host's own committed-result owner, composed into the job source
    /// this state binds.
    #[cfg(not(target_arch = "wasm32"))]
    host_job_source: Option<HostJobSource>,
    /// When this state was built (Unix ms). Fork children whose job started
    /// earlier belonged to a previous process and are re-linked on restore.
    created_at_ms: u64,
    fork_relink_scheduled: std::sync::atomic::AtomicBool,
    /// Mobs whose fork children were already re-linked by this state.
    fork_relinked_mobs: std::sync::Mutex<std::collections::BTreeSet<MobId>>,
    /// Set once the realm-local capability expiry/cleanup driver is running.
    local_forked_participant_sweeper_started: std::sync::atomic::AtomicBool,
    /// Driver cadence, configurable only through the explicit test seam.
    local_forked_participant_sweep_interval_ms: std::sync::atomic::AtomicU64,
    /// Skill sources seeded from the owning mob definition.
    ///
    /// Realm profiles carry skill names, not the source bodies/paths. When an
    /// agent creates a child mob from a realm profile, copy matching source
    /// definitions into the child so profile skills still assemble into the
    /// spawned member's system prompt.
    realm_skill_sources: BTreeMap<String, SkillSource>,
    /// Phase 6b (ADJ-P6B-16): the local-branch member live host passed
    /// through to `MobBuilder::with_member_live_host` for every mob this
    /// console creates/resumes. Late-settable (`set_member_live_host`)
    /// because live transports attach to the RPC router after the mob
    /// state is constructed. Absent ⇒ local `member_live_*` verbs reject
    /// typed `LiveTransportUnavailable` (honest degradation).
    member_live_host:
        std::sync::RwLock<Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>>,
    /// One host-process reverse-lane acceptor shared by every mob builder in
    /// this surface. Absent is honest fail-closed degradation for mixed-host
    /// edges; surfaces must supply an explicit dialable composition.
    controlling_acceptor: Option<meerkat_mob::ControllingAcceptorConfig>,
    /// The host's fork instruction policy for every mob this state creates
    /// or resumes ([`Self::with_fork_instruction_policy`]).
    fork_instruction_policy: meerkat_core::ForkInstructionPolicy,
    /// The console principal this state serves (phase 5, chokepoint (b) —
    /// DEC-P5E-8). Injected at construction, never ambient (gotcha #19):
    /// every v1 local surface (RPC/REST/stdio/public-MCP/CLI/embedder)
    /// passes `Owner` (A16). v2 bearer auth threads per-connection
    /// `External` principals through this same seam — the deterministic
    /// non-owner test lanes already exercise it (ADJ-P5-10).
    console_principal: MobControlPrincipal,
}

impl MobMcpState {
    /// `console_principal` is REQUIRED (no default): the caller states which
    /// principal this console serves. Local single-user surfaces mint
    /// `MobControlPrincipal::Owner` (A16 — behavior byte-identical to
    /// pre-phase-5); an External principal makes every mob verb subject to its
    /// ControlScope grants.
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        console_principal: MobControlPrincipal,
    ) -> Result<Self, meerkat_runtime::RuntimeDriverError> {
        Self::new_with_runtime_adapter(session_service, None, console_principal)
    }

    pub fn new_with_runtime_adapter(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        console_principal: MobControlPrincipal,
    ) -> Result<Self, meerkat_runtime::RuntimeDriverError> {
        let runtime_adapter = session_service.acquire_runtime_adapter(runtime_adapter)?;
        Ok(Self::from_acquired_runtime_adapter(
            session_service,
            runtime_adapter,
            console_principal,
        ))
    }

    fn from_acquired_runtime_adapter(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        console_principal: MobControlPrincipal,
    ) -> Self {
        let can_deliver_detached = runtime_adapter.is_some();
        Self {
            console_principal,
            session_service,
            runtime_adapter,
            workgraph_service: None,
            default_llm_client: None,
            default_llm_client_provider: None,
            external_tools_provider: None,
            member_external_tools_factory: None,
            member_runtime_registration_hook: None,
            before_activation: std::sync::OnceLock::new(),
            after_destroy: std::sync::OnceLock::new(),
            before_create: std::sync::OnceLock::new(),
            child_tool_bundles: ChildToolBundles::default(),
            additional_child_tool_bundles: std::sync::OnceLock::new(),
            child_mcp_servers: std::sync::RwLock::new(ChildMcpServers::default()),
            tool_consequence_policy_registry: None,
            child_application_tool_policy: None,
            child_spawn_member_customizer: None,
            persistent_storage_root: None,
            persistent_storage_setup_error: None,
            mobs: Arc::new(RwLock::new(BTreeMap::new())),
            retired_mobs: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            held_child_mobs: RwLock::new(BTreeMap::new()),
            unrestored_mobs: RwLock::new(BTreeMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
            held_child_transfer: Mutex::new(()),
            #[cfg(test)]
            teardown_preflight_hold: std::sync::Mutex::new(None),
            mob_set_restored: std::sync::atomic::AtomicBool::new(false),
            handles_inserted: std::sync::atomic::AtomicBool::new(false),
            mob_set_epoch: tokio::sync::watch::Sender::new(0),
            implicit_mob_locks: Mutex::new(HashMap::new()),
            workgraph_flow_custodies: Mutex::new(HashMap::new()),
            workgraph_flow_reconcile_epoch: tokio::sync::watch::Sender::new(0),
            workgraph_flow_reconciler_started: std::sync::atomic::AtomicBool::new(false),
            restore_lock: Mutex::new(false),
            realm_profile_store_selection: RealmProfileStoreSelection::DefaultInMemory(Arc::new(
                meerkat_mob::InMemoryRealmProfileStore::new(),
            )),
            forked_participant_store_selection: ForkedParticipantStoreSelection::DefaultInMemory(
                Arc::new(InMemoryForkedParticipantStore::new()),
            ),
            temporary_council_store_selection: TemporaryCouncilStoreSelection::DefaultInMemory(
                Arc::new(InMemoryTemporaryCouncilStore::new()),
            ),
            temporary_council_inflight: std::sync::Mutex::new(HashMap::new()),
            self_weak: std::sync::OnceLock::new(),
            #[cfg(test)]
            create_insert_hold: std::sync::Mutex::new(None),
            #[cfg(test)]
            resume_insert_hold: std::sync::Mutex::new(None),
            #[cfg(test)]
            refused_resume_store: std::sync::Mutex::new(None),
            #[cfg(test)]
            agent_call_hold: std::sync::Mutex::new(None),
            #[cfg(test)]
            member_lookup_hold: std::sync::Mutex::new(None),
            coordinator_id: uuid::Uuid::new_v4().simple().to_string(),
            temporary_council_clock_offset_ms: std::sync::atomic::AtomicI64::new(0),
            temporary_council_clock_changes: tokio::sync::watch::channel(0).0,
            temporary_council_sweep_passes: tokio::sync::watch::channel(0).0,
            temporary_council_cleanup_budget_ms: std::sync::atomic::AtomicU64::new(
                u64::try_from(temporary_council::TEMPORARY_COUNCIL_CLEANUP_BUDGET.as_millis())
                    .unwrap_or(30_000),
            ),
            temporary_council_recovery_scheduled: std::sync::atomic::AtomicBool::new(false),
            state_tasks: state_tasks::StateTasks::new(),
            // Detached delivery needs a runtime to admit completions, so the
            // default follows the runtime's presence: a host built without one
            // is declared unable to deliver, never silently mismatched.
            detached_completion_delivery: std::sync::atomic::AtomicBool::new(can_deliver_detached),
            detached_owner_host: std::sync::RwLock::new(None),
            detached_completions: std::sync::RwLock::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            continuation_binding: std::sync::Mutex::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            host_job_source: None,
            created_at_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            fork_relink_scheduled: std::sync::atomic::AtomicBool::new(false),
            fork_relinked_mobs: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            local_forked_participant_sweeper_started: std::sync::atomic::AtomicBool::new(false),
            local_forked_participant_sweep_interval_ms: std::sync::atomic::AtomicU64::new(
                u64::try_from(LOCAL_FORKED_PARTICIPANT_SWEEP_INTERVAL.as_millis())
                    .unwrap_or(30_000),
            ),
            realm_skill_sources: BTreeMap::new(),
            member_live_host: std::sync::RwLock::new(None),
            controlling_acceptor: None,
            fork_instruction_policy: meerkat_core::ForkInstructionPolicy::default(),
        }
    }

    /// Set the host's fork instruction policy for every mob this state
    /// creates or resumes ([`meerkat_mob::MobBuilder::with_fork_instruction_policy`]).
    /// The default refuses a self-starting fork (`fork_off`, a council
    /// participant) of a member with activated host instructions.
    pub fn with_fork_instruction_policy(
        mut self,
        policy: meerkat_core::ForkInstructionPolicy,
    ) -> Self {
        self.fork_instruction_policy = policy;
        self
    }

    /// Install the process-scoped reverse-lane acceptor used by every mob
    /// created or resumed through this state.
    pub fn with_controlling_acceptor(
        mut self,
        config: meerkat_mob::ControllingAcceptorConfig,
    ) -> Self {
        self.controlling_acceptor = Some(config);
        self
    }

    /// Phase 6b (ADJ-P6B-16): install the local-branch member live host.
    /// Applies to mobs created/resumed AFTER the install (the live plane
    /// attaches before any mob verb is served on the RPC surface).
    pub fn set_member_live_host(
        &self,
        live_host: Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
    ) {
        *self
            .member_live_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(live_host);
    }

    /// Set the shared realm profile store for cross-mob profile CRUD.
    pub fn with_realm_profile_store(
        mut self,
        store: Option<Arc<dyn meerkat_mob::RealmProfileStore>>,
    ) -> Self {
        self.realm_profile_store_selection = RealmProfileStoreSelection::CallerSupplied(store);
        self
    }

    /// Returns a reference to the realm profile store, if configured.
    pub fn realm_profile_store(&self) -> Option<&Arc<dyn meerkat_mob::RealmProfileStore>> {
        self.realm_profile_store_selection.store()
    }

    /// Inject the single realm-scoped forked-participant custody store.
    ///
    /// This affects mobs created or restored by this state only. Handles
    /// manually inserted with [`Self::mob_insert_handle`] remain foreign and
    /// retain the store with which their own builder was composed.
    pub fn with_forked_participant_store(mut self, store: Arc<dyn ForkedParticipantStore>) -> Self {
        self.forked_participant_store_selection =
            ForkedParticipantStoreSelection::CallerSupplied(store);
        self
    }

    /// Return this state's shared realm-scoped capability store.
    ///
    /// This composition seam stays private: public callers inject a store at
    /// construction, while capability access remains on the authorized mob
    /// handle/services rather than exposing bearer-oriented store primitives.
    fn forked_participant_store(&self) -> &Arc<dyn ForkedParticipantStore> {
        self.forked_participant_store_selection.store()
    }

    /// Inject the single realm-scoped temporary-council custody store.
    /// Declare whether this host outlives a tool call long enough to deliver
    /// detached completions (a long-lived server or a keep-alive session).
    /// One-shot hosts declare [`DetachedCompletionDelivery::Unavailable`], so
    /// `fork_off` and `council` block for their result instead of returning a
    /// handle whose completion would be lost when the process exits.
    #[must_use]
    pub fn with_detached_completion_delivery(self, delivery: DetachedCompletionDelivery) -> Self {
        self.set_detached_completion_delivery(delivery);
        self
    }

    /// Re-declare the capability on a shared state, e.g. once a one-shot CLI
    /// run learns it will stay alive.
    pub fn set_detached_completion_delivery(&self, delivery: DetachedCompletionDelivery) {
        if delivery == DetachedCompletionDelivery::Available && self.runtime_adapter.is_none() {
            tracing::error!(
                "detached completion delivery declared Available on a host with no runtime \
                 adapter; fork_off and council will block and report no_runtime_adapter"
            );
        }
        self.detached_completion_delivery.store(
            matches!(delivery, DetachedCompletionDelivery::Available),
            std::sync::atomic::Ordering::Release,
        );
    }

    /// Supply the host hook that makes a plain-session owner of a detached
    /// job (a top-level session that called `council`) live again when the
    /// runtime has retired its executor. Mob member owners are revived
    /// through their mob and need no hook.
    #[must_use]
    pub fn with_detached_owner_host(self, host: Arc<dyn DetachedOwnerHost>) -> Self {
        self.set_detached_owner_host(Some(host));
        self
    }

    /// Replace the plain-session owner hook on a shared state.
    pub fn set_detached_owner_host(&self, host: Option<Arc<dyn DetachedOwnerHost>>) {
        *self
            .detached_owner_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = host;
    }

    /// The plain-session owner hook, if the host supplied one. The host's
    /// continuation resolver uses it to serve a completion to an owner that
    /// is not a mob member.
    pub fn detached_owner_host(&self) -> Option<Arc<dyn DetachedOwnerHost>> {
        self.detached_owner_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Submit this host's detached outcomes through the durable continuation
    /// owner over `inbox` (the host's one runtime delivery inbox), and bind
    /// the mob address resolver and job owner into `bindings` for the host's
    /// delivery owner. The binding lives as long as this state; binding the
    /// same state again is a no-op.
    ///
    /// # Errors
    /// The state has no runtime, or another live state holds `bindings`
    /// (take it over with [`Self::rebind_continuations`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bind_continuations(
        self: &Arc<Self>,
        inbox: meerkat_runtime::RuntimeDeliveryInbox,
        bindings: &meerkat::ContinuationHostBindings,
    ) -> Result<(), BindContinuationsError> {
        self.bind_continuations_with(inbox, |resolver, owner| bindings.bind(resolver, owner))
    }

    /// [`Self::bind_continuations`], taking `bindings` over from the binding
    /// of generation `replaced` (a state this one replaces, still live).
    ///
    /// # Errors
    /// The state has no runtime, or `replaced` is not the current binding.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn rebind_continuations(
        self: &Arc<Self>,
        replaced: meerkat::ContinuationBindingGeneration,
        inbox: meerkat_runtime::RuntimeDeliveryInbox,
        bindings: &meerkat::ContinuationHostBindings,
    ) -> Result<(), BindContinuationsError> {
        self.bind_continuations_with(inbox, |resolver, owner| {
            bindings.rebind(replaced, resolver, owner)
        })
    }

    /// Compose `host` into the job source this state binds, so its committed
    /// results for exactly those `Host` namespaces are confirmed by the same
    /// delivery owner as this state's fork_off and council completions. Set
    /// it before [`Self::bind_continuations`]; a state already bound keeps the
    /// source it bound.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_host_job_source(mut self, host: HostJobSource) -> Self {
        self.host_job_source = Some(host);
        self
    }

    /// The continuation owner service this state bound over the host's one
    /// inbox, with the same address resolver and runtime its fork_off and
    /// council completions use: the host submits its own retained results
    /// through it and reads their status. `None` until bound.
    ///
    /// This is submission and status access only, not a binding or a drain
    /// lease: holding it keeps neither this state's continuation binding
    /// nor the host's delivery owner alive, and it does not retire with
    /// them. Delivery stays governed by the currently bound job source,
    /// address resolver and native authorization.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn continuation_service(&self) -> Option<Arc<meerkat::ContinuationOwnerService>> {
        self.detached_completions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|sink| Arc::clone(&sink.continuations))
    }

    /// The generation of this state's continuation binding, once bound.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn continuation_binding_generation(
        &self,
    ) -> Option<meerkat::ContinuationBindingGeneration> {
        self.continuation_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(meerkat::ContinuationBinding::generation)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bind_continuations_with(
        self: &Arc<Self>,
        inbox: meerkat_runtime::RuntimeDeliveryInbox,
        bind: impl FnOnce(
            Arc<dyn meerkat::ContinuationAddressResolver>,
            Arc<dyn meerkat::RetainedJobSource>,
        )
            -> Result<meerkat::ContinuationBinding, meerkat::ContinuationBindError>,
    ) -> Result<(), BindContinuationsError> {
        let mut held = self
            .continuation_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.is_some() {
            return Ok(());
        }
        let Some(runtime) = self.runtime_adapter.clone() else {
            return Err(BindContinuationsError::NoRuntime);
        };
        let owner = Arc::new(crate::detached_completion_sink::MobJobOwner {
            state: Arc::downgrade(self),
        });
        let resolver: Arc<dyn meerkat::ContinuationAddressResolver> =
            Arc::new(crate::detached_completion_sink::HostContinuationResolver {
                mobs: meerkat_mob::continuation::MobContinuationResolver::new(owner.clone()),
                state: Arc::downgrade(self),
            });
        // The bound source answers for this state's fork_off and council
        // jobs and, for exactly its declared namespaces, the host's own.
        let source: Arc<dyn meerkat::RetainedJobSource> =
            Arc::new(crate::detached_completion_sink::ComposedJobSource {
                native: Arc::clone(&owner),
                host: self.host_job_source.clone(),
            });
        let binding =
            bind(Arc::clone(&resolver), source).map_err(BindContinuationsError::Binding)?;
        let continuations = Arc::new(meerkat::ContinuationOwnerService::new(
            inbox,
            resolver,
            Arc::clone(&runtime),
        ));
        *self
            .detached_completions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(crate::detached_completion_sink::DetachedCompletionSink {
                continuations,
                jobs: owner,
                runtime,
            });
        *held = Some(binding);
        Ok(())
    }

    /// Start `work` as owned work of this state: a stand-down joins it and
    /// never aborts it, as it does a council run. Tests only. `false` when
    /// the state no longer admits owned work; nothing was started then.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_owned_work_for_tests(
        &self,
        name: &'static str,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> bool {
        self.state_tasks
            .spawn(state_tasks::StateTaskKind::Owned, name, work)
            .is_some()
    }

    /// Submit a detached job's outcome through this host's continuation
    /// owner exactly as the live custodian and the re-link do. Tests only.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn submit_detached_completion_for_tests(
        &self,
        owner_member: Option<(MobHandle, meerkat_mob::AgentIdentity)>,
        owner_session_id: &SessionId,
        tool: &'static str,
        job_id: &str,
        status: meerkat_core::event::BackgroundJobTerminalStatus,
        outcome: serde_json::Value,
    ) -> Result<crate::detached_delivery::DetachedCompletionDelivered, String> {
        let sink = self
            .detached_delivery_route()
            .map_err(|unavailable| format!("{unavailable:?}"))?;
        let owner = match owner_member {
            Some((handle, identity)) => {
                crate::detached_delivery::DetachedCompletionOwner::Member(handle, identity)
            }
            None => crate::detached_delivery::DetachedCompletionOwner::Session,
        };
        let result_digest = meerkat_mob::detached_outcome_digest(&outcome);
        sink.submit(
            &owner,
            owner_session_id,
            tool,
            job_id,
            status,
            &outcome,
            &result_digest,
        )
        .await
        .map_err(|error| error.to_string())
    }

    /// Declare that this host has inserted every mob it restores
    /// ([`Self::mob_insert_handle`]): from now on a mob it does not manage is
    /// gone for good, and continuations owed to its members are stranded
    /// instead of waiting. A host with a persistent mob root needs no
    /// declaration; its restore finishing is the same fact.
    ///
    /// A host that inserts mob handles without a persistent root must call
    /// this once its restore is done: until it does, continuations owed to
    /// members of a mob it never inserts again (one destroyed by an earlier
    /// process) stay unserved instead of stranding, and on a governed runtime
    /// a fork_off completion whose job no managed mob holds stays pending
    /// instead of being refused.
    pub fn declare_mob_set_restored(&self) {
        self.mob_set_restored
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether mob `mob_id` is gone for good: this state destroyed it, or it
    /// is not managed although the managed set is complete.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) async fn mob_is_retired(&self, mob_id: &MobId) -> bool {
        if self.mobs.read().await.contains_key(mob_id) {
            return false;
        }
        self.mob_set_complete()
            || self
                .retired_mobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(mob_id)
    }

    /// Whether this state still owes the storage cleanup of a mob it
    /// destroyed under `mob_id`.
    fn destroy_cleanup_owed(&self, mob_id: &MobId) -> bool {
        self.retired_mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .is_some_and(|retired| retired.cleanup_owed())
    }

    /// Whether this state still owes the storage cleanup of a mob it
    /// destroyed whose database is at `path`.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn destroy_cleanup_owed_at(&self, path: &Path) -> bool {
        self.retired_mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .any(|retired| {
                retired
                    .storage
                    .as_ref()
                    .is_some_and(|storage| storage.path == path)
                    && retired.cleanup_owed()
            })
    }

    /// The refusal of an id whose destroyed mob's storage cleanup is still
    /// owed: a mob created or inserted under it would use that storage.
    fn owed_destroy_cleanup_error(mob_id: &MobId) -> MobError {
        MobError::Internal(format!(
            "mob {mob_id} was destroyed and its storage cleanup is still outstanding; \
             retry that destroy before reusing the id"
        ))
    }

    /// How a destroy of `mob_id` by `caller` meets the record of the mobs
    /// this state destroyed. Synchronous apart from the managed-set read:
    /// it never asks an actor, so it still answers once the actor exited.
    ///
    /// A destroy that still owes its storage cleanup comes first, also while
    /// another mob is managed under the id: only its own caller continues it,
    /// and no request destroys that other mob in its place. A record that
    /// owes nothing answers its caller only while no mob is managed under the
    /// id; it never shadows a mob managed there.
    pub(crate) async fn retired_destroy_lookup(
        &self,
        mob_id: &MobId,
        caller: &MobDestroyCaller,
    ) -> RetiredDestroyLookup {
        let managed = self.mobs.read().await.contains_key(mob_id);
        let retired = self
            .retired_mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .cloned();
        match retired {
            Some(retired) if retired.cleanup_owed() => {
                if retired.caller == *caller {
                    RetiredDestroyLookup::Caller(retired)
                } else {
                    RetiredDestroyLookup::OtherCaller
                }
            }
            Some(retired) if !managed && retired.caller == *caller => {
                RetiredDestroyLookup::Caller(retired)
            }
            _ => RetiredDestroyLookup::Ordinary,
        }
    }

    /// Continue the destroy recorded as `retired` for its own caller: settle
    /// the storage cleanup it still owes and answer with its report. A retry
    /// of an outstanding cleanup runs the host's release hook again first.
    pub(crate) async fn continue_retired_destroy(
        &self,
        mob_id: &MobId,
        retired: &RetiredMob,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        let after_destroy = self.after_destroy.get();
        if retired.cleanup_owed() {
            run_after_destroy(after_destroy, mob_id);
        }
        Self::settle_retired_destroy(&self.mobs, mob_id, retired, after_destroy).await
    }

    /// Settle what the destroy recorded as `retired` still owes its storage
    /// and answer with its canonical report: the report itself once nothing
    /// is owed, otherwise `Incomplete` with a copy that names what is still
    /// outstanding. `after_destroy` is run again before each retried drain.
    async fn settle_retired_destroy(
        mobs: &RwLock<BTreeMap<MobId, ManagedMob>>,
        mob_id: &MobId,
        retired: &RetiredMob,
        after_destroy: Option<&MobAfterDestroy>,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        match Self::settle_owed_storage_cleanup(mobs, mob_id, retired, after_destroy).await {
            Ok(()) => Ok(retired.report.clone()),
            Err(outstanding) => Err(MobMcpDestroyError::Incomplete {
                report: retired.report_with_outstanding_cleanup(&outstanding),
            }),
        }
    }

    /// One attempt at the storage cleanup a natively completed destroy owes,
    /// while no other attempt runs. It waits for the mob's actor task to
    /// end, refuses while a mob is managed under the id again, drains this
    /// process's observer of the database, and removes the database and its
    /// sidecars only while the database is the one the mob used. `Err` says
    /// what is still outstanding; the record keeps everything a later
    /// attempt needs.
    ///
    /// A drain refused because other owner handles still observe the
    /// database is retried [`OBSERVER_DRAIN_RETRIES`] times,
    /// [`OBSERVER_DRAIN_RETRY_DELAY`] apart, each after `after_destroy` ran
    /// again: a holder that outlives the hook only briefly, such as an
    /// observer that cloned the handle just before the destroy, then lets go
    /// within the attempt.
    ///
    /// This settles the observer and the files only. A read already entered
    /// through a handle that was then dropped, an opener that attaches after
    /// the drain, and other processes stay outside it, so it never states
    /// that deleting the storage was safe.
    #[cfg(not(target_arch = "wasm32"))]
    async fn settle_owed_storage_cleanup(
        mobs: &RwLock<BTreeMap<MobId, ManagedMob>>,
        mob_id: &MobId,
        retired: &RetiredMob,
        after_destroy: Option<&MobAfterDestroy>,
    ) -> Result<(), String> {
        let Some(storage) = retired.storage.as_ref() else {
            return Ok(());
        };
        let mut owed = storage.owed.lock().await;
        let Some(cleanup) = owed.as_mut() else {
            return Ok(());
        };
        match cleanup.actor_task.wait().await {
            meerkat_mob::MobActorTaskWait::Returned => {}
            meerkat_mob::MobActorTaskWait::Lost => {
                return Err(
                    "the mob's actor task ended without returning, so what it held is not attested"
                        .to_string(),
                );
            }
            meerkat_mob::MobActorTaskWait::Pending => {
                return Err("the mob's actor task has not ended".to_string());
            }
            _ => return Err("how the mob's actor task ended is not known".to_string()),
        }
        if mobs.read().await.contains_key(mob_id) {
            return Err("a mob is managed under this id again".to_string());
        }
        // Only the destroyed mob's own database is drained. Once it is gone
        // (an earlier attempt removed it), there is no database at the path
        // to observe, and only leftover sidecars remain.
        if Self::destroyed_database_at(&storage.path, cleanup.before).await? {
            let mut retries_left = OBSERVER_DRAIN_RETRIES;
            loop {
                let path = storage.path.clone();
                let drained = tokio::task::spawn_blocking(move || {
                    meerkat_mob::SqliteMobStores::drain_observers(&path)
                })
                .await
                .map_err(|error| format!("the observer drain did not return: {error}"))?;
                // A refused drain drained nothing, so it can be retried.
                if retries_left > 0
                    && matches!(
                        drained,
                        Err(meerkat_mob::SqliteMobObserverDrainRefused::OtherOwners { .. })
                    )
                {
                    retries_left -= 1;
                    run_after_destroy(after_destroy, mob_id);
                    tokio::time::sleep(OBSERVER_DRAIN_RETRY_DELAY).await;
                    continue;
                }
                Self::observer_drain_settled(drained)?;
                break;
            }
        }
        Self::remove_destroyed_storage_files(&storage.path, cleanup.before).await?;
        *owed = None;
        Ok(())
    }

    /// Whether `path` still holds the destroyed mob's database (`Ok(true)`)
    /// or nothing (`Ok(false)`). Another file there is `Err`: it is never
    /// drained or removed.
    #[cfg(not(target_arch = "wasm32"))]
    async fn destroyed_database_at(path: &Path, before: StorageFileSeen) -> Result<bool, String> {
        let now = StorageFileSeen::look(path).await.map_err(|error| {
            format!(
                "mob storage '{}' could not be looked at: {error}",
                path.display()
            )
        })?;
        match (before, now) {
            (_, StorageFileSeen::Absent) => Ok(false),
            (StorageFileSeen::Present(before), StorageFileSeen::Present(now)) if before == now => {
                Ok(true)
            }
            _ => Err(format!(
                "mob storage '{}' holds a file other than the destroyed mob's database; \
                 nothing was removed",
                path.display()
            )),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn settle_owed_storage_cleanup(
        _mobs: &RwLock<BTreeMap<MobId, ManagedMob>>,
        _mob_id: &MobId,
        _retired: &RetiredMob,
        _after_destroy: Option<&MobAfterDestroy>,
    ) -> Result<(), String> {
        // A destroyed mob owns no storage files on wasm32.
        Ok(())
    }

    /// Whether a drain of the destroyed mob's database observer left
    /// nothing of it that touches the database running in this process: no
    /// observer was live, or it was closed with its catch-up settled, every
    /// worker joined and no replacement attached. A notification
    /// registration the operating system still holds
    /// (`retained_registrations`) never reads or writes the database, so it
    /// does not keep the cleanup owed. Otherwise what is still open.
    #[cfg(not(target_arch = "wasm32"))]
    fn observer_drain_settled(
        drained: Result<
            meerkat_mob::SqliteMobObserverDrain,
            meerkat_mob::SqliteMobObserverDrainRefused,
        >,
    ) -> Result<(), String> {
        match drained {
            Ok(meerkat_mob::SqliteMobObserverDrain::NotFound) => Ok(()),
            Ok(meerkat_mob::SqliteMobObserverDrain::Closed(report))
                if report.catch_up_settled
                    && report.unjoined_workers == 0
                    && report.joining_elsewhere == 0
                    && !report.replacement_registered =>
            {
                Ok(())
            }
            Ok(meerkat_mob::SqliteMobObserverDrain::Closed(report)) => {
                Err(format!("the observer drain left work open: {report:?}"))
            }
            Err(refused) => Err(format!("the observer drain refused: {refused}")),
        }
    }

    /// Whether the managed set holds every mob this host will ever have
    /// from before: the persistent restore finished, or the host declared
    /// its own restore done ([`Self::declare_mob_set_restored`]). Until then
    /// a mob this state does not manage may still come back.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn mob_set_complete(&self) -> bool {
        self.mob_set_restored
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// The one builder of a managed-set entry. Every insert into the managed
    /// set takes its value from here, under the set's write guard, and
    /// inserts it right away. A host insert sets `handles_inserted` here,
    /// under that guard and before the handle is visible, so a member lookup
    /// that could see the handle reads the latch set; this state's own
    /// creates and its persistent restore never set it (see
    /// [`Self::member_lookup_complete`]).
    fn managed_mob_entry(
        &self,
        handle: MobHandle,
        storage_path: Option<PathBuf>,
        origin: ManagedInsertOrigin,
    ) -> ManagedMob {
        match origin {
            ManagedInsertOrigin::HostInsert => self
                .handles_inserted
                .store(true, std::sync::atomic::Ordering::Release),
            ManagedInsertOrigin::OwnCreate | ManagedInsertOrigin::RootRestore => {}
        }
        ManagedMob {
            handle,
            storage_path,
        }
    }

    /// Whether this state hosts every mob whose roster could name a session:
    /// its mob set is complete ([`Self::mob_set_complete`]), or it has no
    /// persistent root and no handle was ever inserted, so it hosts only the
    /// mobs it created in this process and holds no child. The second arm
    /// holds only while no host inserts a handle: every inserted handle goes
    /// through [`Self::try_mob_insert_handle`], which sets the latch before
    /// the handle is visible, and a lookup reads the answer again after its
    /// scan. Complete means complete over the mobs hosted when the lookup
    /// runs: a member of a mob a host has not inserted yet is not found, so a
    /// host inserts a mob's handle before its members can run turns.
    fn member_lookup_complete(&self) -> bool {
        self.mob_set_complete()
            || (self.persistent_storage_root.is_none()
                && !self
                    .handles_inserted
                    .load(std::sync::atomic::Ordering::Acquire))
    }

    /// Every hosted roster that names `session_id` as a member's session: each
    /// managed mob's, read from its handle's own roster state, and each held
    /// child mob's, read from its store without activation. The roster of a
    /// persisted mob left unrestored ([`Self::unrestored_mobs`]) is read from
    /// its store too, but never yields a match, since that mob's incarnation
    /// is in doubt: when it names the session, or cannot be read, the lookup
    /// refuses [`MobError::ProvenanceUnavailable`] naming that mob. It is one
    /// complete lookup and never answers from part of the hosted set. It
    /// refuses [`MobError::ProvenanceUnavailable`] naming no mob while the
    /// hosted set may be incomplete: the mob set is not restored, and this
    /// state has a persistent root or a host inserted a handle into it,
    /// before the scan or while it ran (a host that inserts handles declares
    /// its restore done with [`Self::declare_mob_set_restored`]). A held
    /// child whose roster cannot be read, or whose roster names the session
    /// but whose incarnation is not recorded yet (a store from before 0.9.4
    /// that no 0.9.4 runtime has resumed), refuses it naming that child. A
    /// held child's store is read once per hold. The lookup reads the mobs
    /// hosted when it runs: a member of a mob a host has not inserted yet is
    /// not found, so a host inserts a mob's handle before its members can run
    /// turns.
    pub async fn session_member_matches(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionMemberMatch>, MobError> {
        self.ensure_restored().await?;
        if !self.member_lookup_complete() {
            return Err(MobError::ProvenanceUnavailable { mob_id: None });
        }
        #[cfg(test)]
        {
            let hold = self
                .member_lookup_hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
        }
        let mut matches: BTreeMap<MobId, SessionMemberMatch> = BTreeMap::new();
        // Held children first and then the managed set, under the transfer
        // lock: a child moving from one set to the other is read in one of
        // them, and a mob read in both counts once.
        #[cfg(not(target_arch = "wasm32"))]
        let _transfer = self.held_child_transfer.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let held = self
                .held_child_mobs
                .read()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for held in held {
                if let Some(found) = Self::held_child_member_of_session(&held, session_id).await? {
                    matches.insert(held.mob_id.clone(), found);
                }
            }
        }
        // A persisted mob left unrestored is hosted too: its stored roster
        // refuses the lookup when it names the session or cannot be read.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let unrestored = self
                .unrestored_mobs
                .read()
                .await
                .iter()
                .map(|(path, unrestored)| (path.clone(), unrestored.mob_id.clone()))
                .collect::<Vec<_>>();
            for (path, mob_id) in unrestored {
                Self::unrestored_member_lookup(&path, &mob_id, session_id).await?;
            }
        }
        let managed = self
            .mobs
            .read()
            .await
            .iter()
            .map(|(mob_id, managed)| (mob_id.clone(), managed.handle.clone()))
            .collect::<Vec<_>>();
        for (mob_id, handle) in managed {
            if let Some((identity, generation)) = handle.member_of_session(session_id).await {
                matches.insert(
                    mob_id.clone(),
                    SessionMemberMatch {
                        mob_id,
                        incarnation: handle.incarnation(),
                        identity,
                        generation,
                    },
                );
            }
        }
        // A handle inserted while the scan ran may hold a roster the scan did
        // not read: the lookup is complete only if it still is now.
        if !self.member_lookup_complete() {
            return Err(MobError::ProvenanceUnavailable { mob_id: None });
        }
        Ok(matches.into_values().collect())
    }

    /// Whether the creation record of the mob `handle` serves proves that
    /// `session_id` created that mob with its own create capability, as the
    /// session is bound now. A session's persisted operator authority binds
    /// no mob, so after a restart the agent mob tools re-derive the manage
    /// grant that create returned from this record instead. The record is
    /// read at the incarnation `handle` serves: a record of an earlier
    /// incarnation proves nothing.
    ///
    /// The record proves it only for a create the session made itself (the
    /// agent mob tools' `mob_create`):
    /// - `MobProvenance::OperatorSession { session_id }`: the session created
    ///   the mob while no hosted roster named it;
    /// - `MobProvenance::OperatorMember`: the session created the mob as the
    ///   member `creator_identity` at `creator_generation` of the parent mob
    ///   at `parent_incarnation`. The generation rule: the record proves only
    ///   that member incarnation, so the parent must be hosted at that
    ///   incarnation and the session must still be the bridge session of
    ///   exactly that member at exactly that generation. A later generation
    ///   of the identity (a reset or a respawn), a re-created parent, or a
    ///   parent that is not hosted proves nothing, even over the same
    ///   session.
    ///
    /// A create a host made for the session
    /// ([`Self::mob_create_definition_with_owner_bridge_session`], recorded
    /// as `Session` or `Member`) granted the session nothing, so a mob the
    /// session merely owns is never widened into its scope. `Host`,
    /// `Ambiguous` and `Unrecorded` name no creator session, and a record
    /// that cannot be read proves nothing: every one answers `false`.
    pub(crate) async fn session_is_recorded_creator(
        &self,
        handle: &MobHandle,
        session_id: &SessionId,
    ) -> bool {
        let provenance = match handle.provenance().await {
            Ok(provenance) => provenance,
            Err(error) => {
                tracing::warn!(
                    mob_id = %handle.mob_id(),
                    %error,
                    "a mob's creation record could not be read to re-derive its creator's grant"
                );
                return false;
            }
        };
        match provenance {
            meerkat_mob::MobProvenance::OperatorSession {
                session_id: creator,
            } => creator == *session_id,
            meerkat_mob::MobProvenance::OperatorMember {
                parent_mob_id,
                parent_incarnation,
                creator_identity,
                creator_generation,
                owner_session,
            } => {
                if owner_session != *session_id {
                    return false;
                }
                let Ok(parent) = self.handle_for(&parent_mob_id).await else {
                    return false;
                };
                parent.incarnation() == parent_incarnation
                    && parent.member_of_session(session_id).await
                        == Some((creator_identity, creator_generation))
            }
            _ => false,
        }
    }

    /// The member of a held child's stored roster whose session is
    /// `session_id`, with the child's recorded incarnation. The child's store
    /// is opened and read once per hold: the first read that succeeds is kept
    /// on the held entry and answers every later lookup. A read that fails is
    /// not kept and, like a match without a recorded incarnation, refuses
    /// with [`MobError::ProvenanceUnavailable`] naming the child.
    #[cfg(not(target_arch = "wasm32"))]
    async fn held_child_member_of_session(
        held: &HeldChildMob,
        session_id: &SessionId,
    ) -> Result<Option<SessionMemberMatch>, MobError> {
        let unavailable = |error: Option<&dyn std::fmt::Display>| {
            match error {
                Some(error) => tracing::warn!(
                    mob_id = %held.mob_id,
                    %error,
                    "held child mob roster could not be read for a provenance lookup"
                ),
                None => tracing::warn!(
                    mob_id = %held.mob_id,
                    "held child mob roster names the session but has no recorded incarnation"
                ),
            }
            MobError::ProvenanceUnavailable {
                mob_id: Some(held.mob_id.clone()),
            }
        };
        let read = match held.roster_read.get() {
            Some(read) => read,
            None => {
                let storage = MobStorage::persistent(&held.storage_path)
                    .map_err(|error| unavailable(Some(&error)))?;
                let members = storage
                    .stored_members_by_session()
                    .await
                    .map_err(|error| unavailable(Some(&error)))?;
                let incarnation = storage
                    .creation_incarnation()
                    .await
                    .map_err(|error| unavailable(Some(&error)))?;
                // Lookups read held children under the transfer lock, so no
                // other read kept one meanwhile.
                held.roster_read.get_or_init(|| HeldChildRosterRead {
                    members,
                    incarnation,
                })
            }
        };
        let Some((identity, generation)) = read.members.get(session_id) else {
            return Ok(None);
        };
        let incarnation = read.incarnation.ok_or_else(|| unavailable(None))?;
        Ok(Some(SessionMemberMatch {
            mob_id: held.mob_id.clone(),
            incarnation,
            identity: identity.clone(),
            generation: *generation,
        }))
    }

    /// Every mob this state manages, with its raw handle, without console
    /// authority gating: the job owner and the continuation resolver read
    /// committed job records and rosters through it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn managed_mob_handles(&self) -> Vec<(MobId, MobHandle)> {
        self.mobs
            .read()
            .await
            .iter()
            .map(|(id, managed)| (id.clone(), managed.handle.clone()))
            .collect()
    }

    /// The custody binding of the council bound to `job_id`, if any.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn council_job_binding(
        &self,
        job_id: &str,
    ) -> Result<Option<meerkat_mob::temporary_council::TemporaryCouncilJobBinding>, String> {
        Ok(self
            .temporary_council_store()
            .list_all()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter_map(|record| record.detached_job)
            .find(|job| job.job_id == job_id))
    }

    /// Where this host submits detached outcomes, or why detached delivery is
    /// unavailable. A host that declares delivery but has no runtime, or never
    /// bound its delivery inbox, is reported, never silently treated as able
    /// to deliver.
    pub(crate) fn detached_delivery_route(
        &self,
    ) -> crate::detached_delivery::DetachedDeliveryRoute {
        if self.detached_completion_delivery() == DetachedCompletionDelivery::Unavailable {
            return Err(
                crate::detached_delivery::DetachedDeliveryUnavailable::HostDeclaredUnavailable,
            );
        }
        if self.runtime_adapter.is_none() {
            return Err(crate::detached_delivery::DetachedDeliveryUnavailable::NoRuntimeAdapter);
        }
        self.detached_completions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(crate::detached_delivery::DetachedDeliveryUnavailable::NoContinuationOwner)
    }

    /// [`Self::detached_delivery_route`] for a call whose result belongs to
    /// `owner_session_id`, with the owner the result goes to. One membership
    /// read decides both: a member of a mob this state manages is revived
    /// through its mob; any other owner is a plain session, revived through
    /// the host's [`DetachedOwnerHost`]. Without that hook the route is
    /// unavailable (`NoOwnerRevivalHost`): the result could not reach the
    /// owner later. An owner whose membership cannot be read is not known to
    /// be a member, so it is treated as a plain session.
    pub(crate) async fn detached_delivery_route_for_owner(
        &self,
        owner_session_id: &SessionId,
    ) -> Result<
        (
            crate::detached_completion_sink::DetachedCompletionSink,
            crate::detached_delivery::DetachedCompletionOwner,
        ),
        crate::detached_delivery::DetachedDeliveryUnavailable,
    > {
        let runtime = self.detached_delivery_route()?;
        let owner = match self
            .member_handle_for_bridge_session(owner_session_id)
            .await
        {
            Ok(Some((_, handle, identity))) => {
                crate::detached_delivery::DetachedCompletionOwner::Member(handle, identity)
            }
            Ok(None) | Err(_) => match self.detached_owner_host() {
                Some(_) => crate::detached_delivery::DetachedCompletionOwner::Session,
                None => {
                    return Err(
                        crate::detached_delivery::DetachedDeliveryUnavailable::NoOwnerRevivalHost,
                    );
                }
            },
        };
        Ok((runtime, owner))
    }

    /// Why fork_off and council would block on this host, or `None` when
    /// they deliver detached.
    pub fn detached_delivery_blocked_because(
        &self,
    ) -> Option<crate::detached_delivery::DetachedDeliveryUnavailable> {
        self.detached_delivery_route().err()
    }

    /// [`Self::detached_delivery_blocked_because`] for a call made by
    /// `owner_session_id`, which also blocks when the result could not reach
    /// that owner later
    /// ([`crate::detached_delivery::DetachedDeliveryUnavailable::NoOwnerRevivalHost`]).
    pub async fn detached_delivery_blocked_because_for(
        &self,
        owner_session_id: &SessionId,
    ) -> Option<crate::detached_delivery::DetachedDeliveryUnavailable> {
        self.detached_delivery_route_for_owner(owner_session_id)
            .await
            .err()
    }

    pub fn detached_completion_delivery(&self) -> DetachedCompletionDelivery {
        if self
            .detached_completion_delivery
            .load(std::sync::atomic::Ordering::Acquire)
        {
            DetachedCompletionDelivery::Available
        } else {
            DetachedCompletionDelivery::Unavailable
        }
    }

    pub fn with_temporary_council_store(mut self, store: Arc<dyn TemporaryCouncilStore>) -> Self {
        self.temporary_council_store_selection =
            TemporaryCouncilStoreSelection::CallerSupplied(store);
        self
    }

    /// Return this state's shared realm-scoped temporary-council store.
    fn temporary_council_store(&self) -> &Arc<dyn TemporaryCouncilStore> {
        self.temporary_council_store_selection.store()
    }

    /// Direct access to the realm-scoped council store, for tests that must
    /// write a crash shape a live coordinator cannot be made to produce.
    #[doc(hidden)]
    pub fn temporary_council_store_for_tests(&self) -> Arc<dyn TemporaryCouncilStore> {
        self.temporary_council_store().clone()
    }

    /// Direct access to the realm-scoped capability store, for tests that must
    /// assert what this realm's custody does and does not hold.
    #[doc(hidden)]
    pub fn forked_participant_store_for_tests(&self) -> Arc<dyn ForkedParticipantStore> {
        self.forked_participant_store().clone()
    }

    /// Trusted in-process temporary-council API (issue #159).
    ///
    /// Public surfaces are thin adapters over this shared coordinator.
    /// Share this state, installing the self-reference that lets persistent
    /// restoration schedule its own temporary-council recovery sweep.
    ///
    /// `Arc::new(state)` still works and keeps every other behaviour, but a
    /// state shared that way has no way to own a background task, so unfinished
    /// councils then need an explicit
    /// [`TemporaryCouncilCoordinator::recover_unfinished`] call.
    #[must_use]
    pub fn into_shared(self) -> Arc<Self> {
        Arc::new_cyclic(|weak| {
            let _ = self.self_weak.set(weak.clone());
            self
        })
    }

    /// Override the bounded temporary-council cleanup budget.
    ///
    /// Cleanup is bounded so a stuck teardown can never hold a joined caller
    /// past the point where an immutable result already exists; the remaining
    /// obligations become explicit durable debt that a later sweep retries.
    #[must_use]
    pub fn with_temporary_council_cleanup_budget(self, budget: std::time::Duration) -> Self {
        self.set_temporary_council_cleanup_budget(budget);
        self
    }

    /// The clock the temporary-council coordinator reads.
    ///
    /// Every wall-clock decision the coordinator makes — deadline resolution,
    /// deadline expiry, claim-lease expiry, cleanup budget, and receipt
    /// timestamps — goes through here, so a test can observe an exact expiry
    /// instead of sleeping and hoping.
    pub(crate) fn temporary_council_now(&self) -> chrono::DateTime<chrono::Utc> {
        let offset = self
            .temporary_council_clock_offset_ms
            .load(std::sync::atomic::Ordering::SeqCst);
        chrono::Utc::now() + chrono::Duration::milliseconds(offset)
    }

    /// Advance (or rewind) the coordinator's clock by `offset`.
    ///
    /// This exists so deadline-expiry and lease-expiry behaviour can be proven
    /// deterministically. It is deliberately not reachable from any surface,
    /// and the default offset is zero.
    #[doc(hidden)]
    pub fn set_temporary_council_clock_offset(&self, offset: chrono::Duration) {
        self.temporary_council_clock_offset_ms.store(
            offset.num_milliseconds(),
            std::sync::atomic::Ordering::SeqCst,
        );
        self.temporary_council_clock_changes
            .send_replace(offset.num_milliseconds());
    }

    /// Wakes when the coordinator's clock offset changes.
    pub(crate) fn temporary_council_clock_changes(&self) -> tokio::sync::watch::Receiver<i64> {
        self.temporary_council_clock_changes.subscribe()
    }

    pub(crate) fn note_temporary_council_sweep_pass(&self) {
        self.temporary_council_sweep_passes
            .send_modify(|passes| *passes = passes.wrapping_add(1));
    }

    /// Passes the post-restore council sweep has finished. Test support:
    /// order work after a sweep pass without polling.
    #[doc(hidden)]
    pub fn temporary_council_sweep_passes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.temporary_council_sweep_passes.subscribe()
    }

    /// Adjust the bounded cleanup budget on a live state.
    pub fn set_temporary_council_cleanup_budget(&self, budget: std::time::Duration) {
        self.temporary_council_cleanup_budget_ms.store(
            u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    pub(crate) fn temporary_council_cleanup_budget(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.temporary_council_cleanup_budget_ms
                .load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    /// Stable per-process coordinator identity used as the council claim id.
    pub(crate) fn coordinator_id(&self) -> &str {
        &self.coordinator_id
    }

    pub fn temporary_council(self: &Arc<Self>) -> TemporaryCouncilCoordinator {
        let _ = self.self_weak.set(Arc::downgrade(self));
        TemporaryCouncilCoordinator::new(self.clone())
    }

    /// Schedule the automatic post-restore council sweep: recovery, the
    /// detached-council re-link, and a retry after the lease of every record
    /// a previous process's claim still held (see [`crate::council_relink`]).
    ///
    /// Runs at most once per state and always on its OWN task, so it can call
    /// back into `ensure_restored`/`handle_for` without re-entering the
    /// restore lock that is held while restoration completes. Only a durable
    /// council store has anything from a previous process to converge.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn schedule_temporary_council_recovery(&self) {
        if self.temporary_council_store().durability()
            != meerkat_mob::temporary_council::TemporaryCouncilStoreDurability::Durable
        {
            return;
        }
        use std::sync::atomic::Ordering;
        if self
            .temporary_council_recovery_scheduled
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        let Some(weak) = self.self_weak.get().cloned() else {
            // No `Arc<Self>` has been observed yet, so nothing can own the
            // sweep. Allow a later attempt rather than pretending it ran.
            self.temporary_council_recovery_scheduled
                .store(false, Ordering::SeqCst);
            return;
        };
        // Built in its own frame: `tokio::spawn` takes the future by value,
        // so an inline sweep future would transit every caller's debug frame.
        let _ = self.state_tasks.spawn(
            state_tasks::StateTaskKind::Convergent,
            "temporary council recovery sweep",
            meerkat_runtime::stack_relief::box_in_own_frame(move || {
                crate::council_relink::restore_sweep(weak)
            }),
        );
    }

    /// Override the local capability sweep cadence for deterministic tests.
    #[doc(hidden)]
    pub fn set_local_forked_participant_sweep_interval(&self, interval: Duration) {
        let interval = interval.max(Duration::from_millis(1));
        self.local_forked_participant_sweep_interval_ms.store(
            u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    fn schedule_local_forked_participant_sweeper(&self) {
        use std::sync::atomic::Ordering;
        if self
            .local_forked_participant_sweeper_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let Some(weak) = self.self_weak.get().cloned() else {
            self.local_forked_participant_sweeper_started
                .store(false, Ordering::SeqCst);
            return;
        };
        let _ = self.state_tasks.spawn(
            state_tasks::StateTaskKind::Periodic,
            "local forked-participant sweeper",
            meerkat_runtime::stack_relief::box_in_own_frame(move || async move {
                loop {
                    let Some(state) = weak.upgrade() else {
                        return;
                    };
                    state.sweep_local_forked_participants().await;
                    let interval = Duration::from_millis(
                        state
                            .local_forked_participant_sweep_interval_ms
                            .load(Ordering::SeqCst),
                    );
                    drop(state);
                    tokio::time::sleep(interval).await;
                }
            }),
        );
    }

    async fn sweep_local_forked_participants(&self) {
        let Some(runtime) = self
            .session_service
            .clone()
            .forked_participant_source_runtime()
        else {
            return;
        };
        let store = self.forked_participant_store().clone();
        let records = match store.list_all().await {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "local forked-participant sweep could not list capability custody"
                );
                return;
            }
        };
        let routes: BTreeSet<_> = records
            .into_iter()
            .map(|record| record.sidecar.owner_route)
            .filter(|route| {
                matches!(
                    route,
                    meerkat_mob::forked_participant::ForkedParticipantOwnerRoute::Local { .. }
                )
            })
            .collect();
        for route in routes {
            let service = match meerkat_mob::forked_participant::ForkedParticipantService::new(
                route,
                store.clone(),
                runtime.clone(),
            ) {
                Ok(service) => service,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "local forked-participant sweep could not compose owner service"
                    );
                    continue;
                }
            };
            if let Err(error) = service.sweep_expiry(chrono::Utc::now()).await {
                tracing::warn!(
                    error = %error,
                    "local forked-participant expiry sweep failed"
                );
            }
            if let Err(error) = service.sweep_cleanup(chrono::Utc::now()).await {
                tracing::warn!(
                    error = %error,
                    "local forked-participant cleanup sweep failed"
                );
            }
        }
    }

    /// The console principal this state serves, for capability verbs that take
    /// an explicit caller.
    fn console_principal_snapshot(&self) -> MobControlPrincipal {
        self.console_principal.clone()
    }

    /// Any managed transport handle other than the named authority.
    ///
    /// The bridge send atomically excludes shutdown; this selection is not a
    /// liveness or authority decision.
    async fn any_managed_handle_except(
        &self,
        excluded: &MobId,
    ) -> Result<Option<MobHandle>, MobError> {
        self.ensure_restored().await?;
        Ok(self
            .mobs
            .read()
            .await
            .iter()
            .find(|(mob_id, _)| *mob_id != excluded)
            .map(|(_, managed)| {
                managed
                    .handle
                    .clone()
                    .with_command_authority(CommandAuthority::principal(
                        self.console_principal.clone(),
                    ))
            }))
    }

    fn temporary_council_reserve_inflight(
        &self,
        council_id: String,
        fingerprint: String,
        completion: tokio::sync::watch::Receiver<Option<temporary_council::CouncilPublish>>,
    ) -> temporary_council::InflightReservation {
        let mut inflight = self
            .temporary_council_inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = inflight.get(&council_id) {
            return temporary_council::InflightReservation::Existing {
                fingerprint: entry.fingerprint.clone(),
                completion: entry.completion.clone(),
            };
        }
        inflight.insert(
            council_id,
            temporary_council::InflightCouncil {
                fingerprint,
                completion,
            },
        );
        temporary_council::InflightReservation::Owned
    }

    fn temporary_council_remove_inflight(&self, council_id: &str) {
        self.temporary_council_inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(council_id);
    }

    /// Configure durable mob storage, returning a typed error when the
    /// explicitly rooted capability database cannot pass SQLite ledger
    /// validation (including a future-schema refusal).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_with_persistent_storage_root(
        mut self,
        runtime_root: Option<PathBuf>,
    ) -> Result<Self, MobError> {
        self.configure_persistent_storage_root(runtime_root)?;
        Ok(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn configure_persistent_storage_root(
        &mut self,
        runtime_root: Option<PathBuf>,
    ) -> Result<(), MobError> {
        let Some(runtime_root) = runtime_root else {
            if let Some(existing_root) = &self.persistent_storage_root {
                return Err(MobError::Internal(format!(
                    "cannot clear persistent mob root '{}' after state construction",
                    existing_root.display()
                )));
            }
            self.persistent_storage_root = None;
            self.persistent_storage_setup_error = None;
            return Ok(());
        };
        let mob_root = Self::persistent_mob_root(&runtime_root);
        if let Some(existing_root) = &self.persistent_storage_root
            && existing_root != &mob_root
        {
            return Err(MobError::Internal(format!(
                "cannot change persistent mob root from '{}' to '{}' after state construction",
                existing_root.display(),
                mob_root.display()
            )));
        }
        std::fs::create_dir_all(&mob_root).map_err(|error| {
            MobError::Internal(format!(
                "failed to create persistent mob root '{}': {error}",
                mob_root.display()
            ))
        })?;
        if self.forked_participant_store_selection.is_default() {
            let db_path = Self::persistent_forked_participant_store_path(&runtime_root);
            let store = SqliteForkedParticipantStore::open(&db_path)?;
            self.forked_participant_store_selection =
                ForkedParticipantStoreSelection::DefaultDurable(Arc::new(store));
        }
        if self.temporary_council_store_selection.is_default() {
            // Same explicitly rooted realm custody database as capability
            // custody: one realm-scoped orchestration file, one schema domain.
            let db_path = Self::persistent_forked_participant_store_path(&runtime_root);
            let store = SqliteTemporaryCouncilStore::open(&db_path)?;
            self.temporary_council_store_selection =
                TemporaryCouncilStoreSelection::DefaultDurable(Arc::new(store));
        }
        if self.realm_profile_store_selection.is_default() {
            let db_path = mob_root.join(Self::REALM_PROFILE_STORE_FILE_NAME);
            let store = meerkat_mob::SqliteRealmProfileStore::open(&db_path)?;
            self.realm_profile_store_selection =
                RealmProfileStoreSelection::DefaultDurable(Arc::new(store));
        }
        self.persistent_storage_root = Some(mob_root);
        self.persistent_storage_setup_error = None;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_persistent_storage_root(mut self, runtime_root: Option<PathBuf>) -> Self {
        if let Err(error) = self.configure_persistent_storage_root(runtime_root.clone()) {
            tracing::warn!(error = %error, "persistent mob state setup failed");
            self.persistent_storage_root =
                runtime_root.map(|root| Self::persistent_mob_root(&root));
            self.persistent_storage_setup_error = Some(error.to_string());
        }
        self
    }

    #[cfg(target_arch = "wasm32")]
    pub fn with_persistent_storage_root(mut self, runtime_root: Option<PathBuf>) -> Self {
        self.persistent_storage_root = runtime_root.map(|root| Self::persistent_mob_root(&root));
        self
    }

    pub fn with_workgraph_service(mut self, service: Option<meerkat::WorkGraphService>) -> Self {
        self.workgraph_service = service;
        self
    }

    /// The host-supplied WorkGraph service, scoped as the host scoped it.
    #[must_use]
    pub fn workgraph_service(&self) -> Option<&meerkat::WorkGraphService> {
        self.workgraph_service.as_ref()
    }

    /// The shared WorkGraph as `mob_id`'s members see it: the host service's
    /// store rescoped to the realm `mob.<id>`. Live delegation schedules its
    /// voice work items through this so the forks' `workgraph_*` tools, which
    /// build in that realm, address the same items. `None` when the host has
    /// no WorkGraph service or its backend is disabled.
    pub fn workgraph_service_for_mob(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<meerkat::WorkGraphService>, MobError> {
        self.workgraph_service
            .as_ref()
            .map(|service| meerkat_mob::mob_scoped_workgraph_service(service, mob_id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn with_default_llm_client(mut self, client: Option<Arc<dyn LlmClient>>) -> Self {
        self.default_llm_client = client;
        self
    }

    pub fn with_default_llm_client_provider(
        mut self,
        provider: Option<DefaultLlmClientProvider>,
    ) -> Self {
        self.default_llm_client_provider = provider;
        self
    }

    pub fn with_external_tools_provider(
        mut self,
        provider: Option<meerkat_mob::ExternalToolsProvider>,
    ) -> Self {
        self.external_tools_provider = provider;
        self
    }

    /// Build each mob's member external tools from its id, for every mob
    /// this state builds without caller-supplied member tools: a restored
    /// mob, and one created other than through
    /// [`Self::mob_create_definition_with_member_external_tools`]. A surface
    /// that binds member tools per mob (an RPC connection's callback route,
    /// which a later [`Self::mob_claim_member_external_tools`] rebinds)
    /// supplies it. Without it, those mobs get
    /// [`Self::with_external_tools_provider`]'s state-wide tools. A build's
    /// [`MemberExternalTools::on_managed`] runs once its mob is managed.
    pub fn with_member_external_tools_factory(
        mut self,
        factory: MemberExternalToolsFactory,
    ) -> Self {
        self.member_external_tools_factory = Some(factory);
        self
    }

    /// The member external tools of the mob `mob_id` builds: the caller's
    /// own, else the per-mob factory's, with the factory's on-managed hook.
    fn member_external_tools_for(
        &self,
        mob_id: &MobId,
        supplied: Option<meerkat_mob::ExternalToolsProvider>,
    ) -> MemberExternalToolsBuild {
        if let Some(supplied) = supplied {
            return (Some(supplied), None);
        }
        match self
            .member_external_tools_factory
            .as_ref()
            .and_then(|factory| factory(mob_id))
        {
            Some(tools) => (Some(tools.provider), tools.on_managed),
            None => (None, None),
        }
    }

    /// Inherit the host's member destination selection into every child mob
    /// this state creates or restores, so no child member registers without
    /// its destination installed.
    pub fn with_member_runtime_registration_hook(
        mut self,
        hook: Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>,
    ) -> Self {
        self.member_runtime_registration_hook = Some(hook);
        self
    }

    /// Host Rust tool bundles for mobs created through the mob tools. Callers
    /// may name only bundles registered as child-available; see
    /// [`ChildToolBundles`]. The default offers none.
    pub fn with_child_tool_bundles(mut self, bundles: ChildToolBundles) -> Self {
        self.child_tool_bundles = bundles;
        self
    }

    /// Bind host services for every child handle before member execution.
    /// Install once before any child create/restore operation is admitted.
    pub fn set_before_activation(
        &self,
        callback: meerkat_mob::MobBeforeActivation,
    ) -> Result<(), MobError> {
        self.before_activation
            .set(callback)
            .map_err(|_| MobError::Internal("before-activation callback already bound".into()))
    }

    /// Release host services bound to a mob this state destroys. Once a
    /// destroy completed natively and the mob left the managed set, the hook
    /// gets the mob's id before the storage cleanup that destroy owes, and
    /// again before each retry of an outstanding cleanup. A handle of the
    /// mob the host still holds then, such as the read handle
    /// [`Self::set_before_activation`] handed it, keeps that cleanup
    /// outstanding, so the host drops it here.
    ///
    /// By the time the hook runs, another mob may be managed under the same
    /// id: release only what observes the destroyed mob, such as a handle
    /// whose phase is `Destroyed`. The first call runs on the destroy's own
    /// task, a retry's on the retrying caller's task; the hook must not
    /// block. A panic in it is caught and logged, and the destroy goes on.
    /// Install once before any destroy is admitted.
    pub fn set_after_destroy(&self, hook: MobAfterDestroy) -> Result<(), MobError> {
        self.after_destroy
            .set(hook)
            .map_err(|_| MobError::Internal("after-destroy hook already bound".into()))
    }

    /// Admit or refuse each mob this state creates. Before anything of a
    /// new mob exists (its storage, its actor, its managed entry), the hook
    /// gets a [`MobCreateRequest`] and answers `Ok(())` to let the create
    /// proceed or `Err(reason)` to refuse it. Every create path asks it: an
    /// agent's `mob_create`, the implicit delegation mob, a temporary
    /// council, a host's definition or mobpack create. Restoring a persisted
    /// mob creates nothing and never asks it, and neither does a handle a
    /// host registers itself ([`Self::mob_insert_handle`],
    /// [`Self::try_mob_insert_handle`]).
    ///
    /// The hook fails closed: a refusal, a panic while calling it and a
    /// panic in the future it returns all refuse the create, with an error
    /// [`Self::is_create_refusal`] recognizes. The reason is in the error's
    /// message and, under `reason`, in its structured data (`kind`
    /// [`MOB_CREATE_REFUSED_KIND`]). Install once before any create is
    /// admitted.
    pub fn set_before_create(&self, hook: MobBeforeCreate) -> Result<(), MobError> {
        self.before_create
            .set(hook)
            .map_err(|_| MobError::Internal("before-create hook already bound".into()))
    }

    /// Whether `error` is a host's refusal of a mob create, through the hook
    /// [`Self::set_before_create`] installs.
    pub fn is_create_refusal(error: &MobError) -> bool {
        matches!(
            error,
            MobError::SessionError(meerkat_core::service::SessionError::FailedWithData {
                data,
                ..
            }) if data.get("kind").and_then(serde_json::Value::as_str)
                == Some(MOB_CREATE_REFUSED_KIND)
        )
    }

    /// The error a refused create answers with: the host's reason in its
    /// message and in its structured data.
    fn create_refused(mob_id: &MobId, reason: &str) -> MobError {
        MobError::SessionError(meerkat_core::service::SessionError::FailedWithData {
            message: format!("mob create refused for mob id '{mob_id}': {reason}"),
            data: json!({
                "kind": MOB_CREATE_REFUSED_KIND,
                "mob_id": mob_id.to_string(),
                "reason": reason,
                "retryable": false,
            }),
        })
    }

    /// Ask the host's create hook, if one is bound, whether the mob
    /// `request` describes may be created. Runs before anything of the mob
    /// exists; see [`Self::set_before_create`].
    async fn admit_create(&self, request: MobCreateRequest) -> Result<(), MobError> {
        let Some(hook) = self.before_create.get() else {
            return Ok(());
        };
        let mob_id = request.mob_id.clone();
        let panicked = || {
            tracing::error!(mob_id = %mob_id, "the host's create admission panicked");
            Err(BEFORE_CREATE_PANICKED.to_string())
        };
        let verdict = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(request)))
        {
            Ok(answer) => {
                match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(answer)).await {
                    Ok(verdict) => verdict,
                    Err(_) => panicked(),
                }
            }
            Err(_) => panicked(),
        };
        verdict.map_err(|reason| Self::create_refused(&mob_id, &reason))
    }

    /// Add provider-opened host bundles before child create/restore is admitted.
    /// This is a one-time bootstrap operation, not a tool-facing mutation.
    pub fn set_additional_child_tool_bundles(
        &self,
        bundles: ChildToolBundles,
    ) -> Result<(), MobError> {
        self.additional_child_tool_bundles
            .set(bundles)
            .map_err(|_| MobError::Internal("additional child bundles already bound".into()))
    }

    /// Host-attested public MCP descriptors for inline child profiles,
    /// including implicit delegate mobs. The default supplies none.
    pub fn with_child_mcp_servers(self, servers: ChildMcpServers) -> Self {
        self.set_child_mcp_servers(servers);
        self
    }

    /// Replace the host supply for future child creation. Existing persisted
    /// profiles retain their descriptors and are revalidated by the host resolver.
    pub fn set_child_mcp_servers(&self, servers: ChildMcpServers) {
        *self
            .child_mcp_servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = servers;
    }

    /// Install the host's tool consequence-policy registry. It is forwarded
    /// to every child mob builder, and installing it makes the host managed:
    /// child mob creation then requires
    /// [`Self::with_child_application_tool_policy`].
    pub fn with_tool_consequence_policy_registry(
        mut self,
        registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    ) -> Self {
        self.tool_consequence_policy_registry = Some(registry);
        self
    }

    /// The application tool policy every member of a child mob is built
    /// with. An explicit [`meerkat_core::ApplicationToolPolicyBinding::Unmanaged`]
    /// is a valid choice; callers can never set or override it.
    pub fn with_child_application_tool_policy(
        mut self,
        binding: meerkat_core::ApplicationToolPolicyBinding,
    ) -> Self {
        self.child_application_tool_policy = Some(binding);
        self
    }

    /// A host [`meerkat_mob::SpawnMemberCustomizer`] for every mob this state
    /// creates or restores, child mobs included: fresh spawns, process-restart
    /// restore and explicit resume (where it is asked through
    /// [`meerkat_mob::SpawnMemberCustomizer::customize_resume`] with the
    /// member's persisted identity).
    ///
    /// It runs before the internal child application tool policy, so in a
    /// child mob the policy always has the last word: a host customizer
    /// cannot widen a child member's application tool policy.
    pub fn with_child_spawn_member_customizer(
        mut self,
        customizer: Arc<dyn meerkat_mob::SpawnMemberCustomizer>,
    ) -> Self {
        self.child_spawn_member_customizer = Some(customizer);
        self
    }

    /// Refuse child mob creation (the agent `mob_create` tool, and the
    /// implicit mob `delegate` helpers run in) up front when the host's child
    /// policy cannot be applied (a managed host without a child policy, a
    /// provider policy without a registry, or `Inherit`).
    pub fn admit_child_tool_policy(&self) -> Result<(), ChildToolPolicyRefused> {
        self.child_tool_policy().map(|_| ())
    }

    fn child_tool_policy(
        &self,
    ) -> Result<meerkat_core::ApplicationToolPolicyBinding, ChildToolPolicyRefused> {
        child_tool_policy::resolve_child_policy(
            self.tool_consequence_policy_registry.as_ref(),
            self.child_application_tool_policy.as_ref(),
        )
    }

    /// Seed skill source definitions available to realm-referenced profiles.
    pub fn with_realm_skill_sources(mut self, sources: BTreeMap<String, SkillSource>) -> Self {
        self.realm_skill_sources = sources;
        self
    }

    async fn hydrate_definition_skill_sources(
        &self,
        definition: &mut MobDefinition,
    ) -> Result<(), MobError> {
        if self.realm_skill_sources.is_empty() {
            return Ok(());
        }

        let mut skill_names = BTreeSet::new();
        for binding in definition.profiles.values() {
            match binding {
                ProfileBinding::Inline(profile) => {
                    skill_names.extend(profile.skills.iter().cloned());
                }
                ProfileBinding::RealmRef { realm_profile } => {
                    let Some(store) = self.realm_profile_store_selection.store() else {
                        continue;
                    };
                    let stored = store.get(realm_profile).await.map_err(|error| {
                        MobError::Internal(format!(
                            "failed to load realm profile '{realm_profile}' while hydrating skill sources: {error}"
                        ))
                    })?;
                    if let Some(stored) = stored {
                        skill_names.extend(stored.profile.skills.iter().cloned());
                    }
                }
            }
        }

        for skill_name in skill_names {
            if definition.skills.contains_key(&skill_name) {
                continue;
            }
            if let Some(source) = self.realm_skill_sources.get(&skill_name) {
                definition.skills.insert(skill_name, source.clone());
            }
        }

        Ok(())
    }

    /// Reserved filename of the realm-scoped profile store inside the mob
    /// persistent root. This database lives in the same directory as the
    /// per-mob `*.db` files but is NOT a mob event log; the restore scan must
    /// never treat it as a mob storage candidate.
    #[cfg(not(target_arch = "wasm32"))]
    const REALM_PROFILE_STORE_FILE_NAME: &str = "realm_profiles.db";
    /// Reserved custody database name; per-mob files are always prefixed with
    /// [`Self::MOB_STORE_FILE_PREFIX`], so a user MobId can never collide.
    #[cfg(not(target_arch = "wasm32"))]
    const REALM_FORKED_PARTICIPANT_STORE_FILE_NAME: &str = "realm_forked_participants.db";
    #[cfg(not(target_arch = "wasm32"))]
    const MOB_STORE_FILE_PREFIX: &str = "mob--";

    fn persistent_mob_root(runtime_root: &Path) -> PathBuf {
        runtime_root.join("mobs")
    }

    /// Explicit durable path for realm-scoped forked-participant custody.
    ///
    /// Surfaces that host capabilities directly (rather than through this
    /// state) use this same rooted location; it is not a per-mob database.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn persistent_forked_participant_store_path(runtime_root: &Path) -> PathBuf {
        Self::persistent_mob_root(runtime_root).join(Self::REALM_FORKED_PARTICIPANT_STORE_FILE_NAME)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn escape_mob_id_for_path(mob_id: &MobId) -> String {
        let mut escaped = String::with_capacity(mob_id.to_string().len() * 3);
        for byte in mob_id.to_string().bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                escaped.push(char::from(byte));
            } else {
                escaped.push('%');
                escaped.push_str(&format!("{byte:02X}"));
            }
        }
        escaped
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn persistent_storage_path(root: &Path, mob_id: &MobId) -> PathBuf {
        root.join(format!(
            "{}{}.db",
            Self::MOB_STORE_FILE_PREFIX,
            Self::escape_mob_id_for_path(mob_id)
        ))
    }

    fn default_llm_client_snapshot(&self) -> Option<Arc<dyn LlmClient>> {
        self.default_llm_client.clone().or_else(|| {
            self.default_llm_client_provider
                .as_ref()
                .and_then(|provider| provider())
        })
    }

    /// The activation hook for a restored mob: classify it from its persisted
    /// owner bridge authority, then run the host's own hook. A cold resume
    /// calls it before any member is restored, so every restored member of a
    /// child mob is built under the child policy.
    #[cfg(not(target_arch = "wasm32"))]
    fn restore_scope_hook(
        &self,
        scope: child_tool_policy::ChildMobScope,
    ) -> meerkat_mob::MobBeforeActivation {
        let host = self.before_activation.get().cloned();
        Arc::new(move |handle: meerkat_mob::MobReadHandle| {
            if handle
                .owner_bridge_session_lifecycle_authority()
                .is_some_and(|authority| {
                    child_tool_policy::is_child_mob(authority.destroy_on_owner_archive)
                })
            {
                scope.mark_child();
            }
            match &host {
                Some(host) => host(handle),
                None => Box::pin(async { Ok(()) }),
            }
        })
    }

    /// The spawn customizer of every mob this state builds: the host's, then
    /// the child policy. Policy last: the child policy overrides whatever the
    /// host customizer set.
    fn member_customizer(
        &self,
        scope: child_tool_policy::ChildMobScope,
    ) -> Arc<dyn meerkat_mob::SpawnMemberCustomizer> {
        let child_policy: Arc<dyn meerkat_mob::SpawnMemberCustomizer> =
            Arc::new(child_tool_policy::ChildPolicyCustomizer {
                policy: self.child_tool_policy(),
                scope,
            });
        match &self.child_spawn_member_customizer {
            Some(host) => Arc::new(
                meerkat_mob::SpawnMemberCustomizerChain::new()
                    .then(Arc::clone(host))
                    .then(child_policy),
            ),
            None => child_policy,
        }
    }

    /// The tool bundles this host offers to child mobs: what a child mob's
    /// inline profile may name in its `tools.bundles` opt-in.
    fn offered_child_tool_bundles(&self) -> Vec<String> {
        let mut offered: BTreeSet<String> = self.child_tool_bundles.offered().cloned().collect();
        if let Some(bundles) = self.additional_child_tool_bundles.get() {
            offered.extend(bundles.offered().cloned());
        }
        offered.into_iter().collect()
    }

    /// Refuse a definition whose inline profile opts into a tool bundle this
    /// create does not offer: a child create offers what the host offers to
    /// child mobs, any other create offers none. Realm-referenced profiles
    /// are host-stored and keep what the host named.
    fn refuse_unoffered_tool_bundles(
        &self,
        definition: &MobDefinition,
        child: bool,
    ) -> Result<(), MobError> {
        let offered = if child {
            self.offered_child_tool_bundles()
        } else {
            Vec::new()
        };
        for (profile_name, binding) in &definition.profiles {
            let Some(profile) = binding.as_inline() else {
                continue;
            };
            if let Some(bundle) = profile
                .tools
                .bundles
                .iter()
                .find(|bundle| !offered.contains(bundle))
            {
                return Err(MobError::ToolBundleNotOffered {
                    profile: profile_name.clone(),
                    bundle: bundle.clone(),
                    offered,
                });
            }
        }
        Ok(())
    }

    fn configure_builder(
        &self,
        mut builder: MobBuilder,
        scope: child_tool_policy::ChildMobScope,
        member_external_tools: Option<meerkat_mob::ExternalToolsProvider>,
    ) -> MobBuilder {
        if let Some(hook) = self.member_runtime_registration_hook.as_ref() {
            builder = builder.with_member_runtime_registration_hook(Arc::clone(hook));
        }
        builder = builder
            .with_session_service(self.session_service.clone())
            .allow_ephemeral_sessions(!self.session_service.supports_persistent_sessions())
            .with_default_external_tools_provider(
                member_external_tools.or_else(|| self.external_tools_provider.clone()),
            )
            .with_workgraph_service(self.workgraph_service.clone());
        builder = self.child_tool_bundles.configure(builder);
        if let Some(bundles) = self.additional_child_tool_bundles.get() {
            builder = bundles.configure(builder);
        }
        if let Some(callback) = self.before_activation.get() {
            builder = builder.before_activation(Arc::clone(callback));
        }
        if let Some(registry) = &self.tool_consequence_policy_registry {
            builder = builder.with_tool_consequence_policy_registry(Arc::clone(registry));
        }
        builder = builder.with_spawn_member_customizer(self.member_customizer(scope));
        if let Some(adapter) = &self.runtime_adapter {
            builder = builder.with_runtime_adapter(adapter.clone());
        }
        if let Some(client) = self.default_llm_client_snapshot() {
            builder = builder.with_default_llm_client(client);
        }
        if let Some(live_host) = self
            .member_live_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            builder = builder.with_member_live_host(live_host);
        }
        if let Some(acceptor) = &self.controlling_acceptor {
            builder = builder.with_controlling_acceptor(acceptor.clone());
        }
        builder.with_fork_instruction_policy(self.fork_instruction_policy)
    }

    fn bind_realm_profile_store(&self, storage: MobStorage) -> MobStorage {
        storage
            .with_realm_profile_store(self.realm_profile_store_selection.store().cloned())
            .with_forked_participant_store(Some(self.forked_participant_store().clone()))
    }

    async fn storage_for_new_mob(
        &self,
        mob_id: &MobId,
    ) -> Result<(MobStorage, Option<PathBuf>), MobError> {
        #[cfg(target_arch = "wasm32")]
        let _ = mob_id;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = &self.persistent_storage_root {
            // Mob event-log persistence is owned by this explicitly bound
            // storage root, independently of whether member session bodies
            // are durable. Coupling the two capabilities makes a legitimate
            // ephemeral session service silently downgrade the Mob itself to
            // in-memory storage even though its durable root was configured.
            tokio::fs::create_dir_all(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to create persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;
            let path = Self::persistent_storage_path(root, mob_id);
            let storage = self.bind_realm_profile_store(MobStorage::persistent(&path)?);
            return Ok((storage, Some(path)));
        }

        Ok((self.bind_realm_profile_store(MobStorage::in_memory()), None))
    }

    /// Remove the mob's durable storage files. Fails CLOSED: a surviving
    /// db file after destroy is durable truth contradicting the reported
    /// terminal state, so the caller must surface the failure rather than
    /// report a clean destroy over a still-present store.
    async fn remove_storage_files(path: Option<&Path>) -> Result<(), MobError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = path else { return Ok(()) };
            let paths = Self::storage_file_paths(path);

            for path in &paths {
                Self::remove_one_storage_file(path).await?;
            }
            // Removal succeeded; verify the primary db path is actually
            // gone so a concurrent by-path re-open cannot silently
            // resurrect it behind a clean destroy report.
            match tokio::fs::metadata(&paths[0]).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(MobError::Internal(format!(
                    "mob storage '{}' post-remove verification failed: {error}",
                    paths[0].display()
                ))),
                Ok(_) => Err(MobError::Internal(format!(
                    "mob storage '{}' still exists after removal (recreated by a live handle?)",
                    paths[0].display()
                ))),
            }
        }
    }

    /// A mob database and its WAL and shared-memory sidecars.
    #[cfg(not(target_arch = "wasm32"))]
    fn storage_file_paths(path: &Path) -> [PathBuf; 3] {
        let sidecar = |suffix: &str| {
            let mut value = path.as_os_str().to_os_string();
            value.push(suffix);
            PathBuf::from(value)
        };
        [path.to_path_buf(), sidecar("-wal"), sidecar("-shm")]
    }

    /// Remove a destroyed mob's database and its sidecars, and verify all
    /// three are gone. The database is removed only while the file at
    /// `path` is still the one the mob used (`before`); when the database is
    /// already gone, only leftover sidecars are removed. When another file
    /// took the path, nothing is removed.
    ///
    /// Looking at the file and unlinking it by path are two steps: a file
    /// put there in between by a process or opener outside this state is
    /// not excluded.
    #[cfg(not(target_arch = "wasm32"))]
    async fn remove_destroyed_storage_files(
        path: &Path,
        before: StorageFileSeen,
    ) -> Result<(), String> {
        let remove_database = Self::destroyed_database_at(path, before).await?;
        let paths = Self::storage_file_paths(path);
        let first = usize::from(!remove_database);
        for path in &paths[first..] {
            Self::remove_one_storage_file(path)
                .await
                .map_err(|error| error.to_string())?;
        }
        for path in &paths {
            match tokio::fs::symlink_metadata(path).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "mob storage '{}' post-remove verification failed: {error}",
                        path.display()
                    ));
                }
                Ok(_) => {
                    return Err(format!(
                        "mob storage '{}' still exists after removal",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn remove_one_storage_file(path: &Path) -> Result<(), MobError> {
        let mut last_error = None;
        let mut delay = Duration::from_millis(10);
        for attempt in 0..5 {
            match tokio::fs::remove_file(path).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 4 {
                        ::tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2);
                    }
                }
            }
        }
        Err(MobError::Internal(format!(
            "failed to remove mob storage file '{}': {}",
            path.display(),
            last_error.map_or_else(|| "unknown error".to_string(), |error| error.to_string())
        )))
    }

    async fn restore_from_persistent_storage(&self) -> Result<(), MobError> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(root) = &self.persistent_storage_root else {
                return Ok(());
            };
            // Each pass finds the stores whose resume fails afresh: a store
            // an earlier pass left unrestored and that resumes now is
            // managed, never still listed as unrestored.
            self.unrestored_mobs.write().await.clear();
            tokio::fs::create_dir_all(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to create persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;

            let mut dir = tokio::fs::read_dir(root).await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to read persistent mob root '{}': {error}",
                    root.display()
                ))
            })?;

            while let Some(entry) = dir.next_entry().await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to iterate persistent mob root '{}': {error}",
                    root.display()
                ))
            })? {
                let path = entry.path();
                let file_type = entry.file_type().await.map_err(|error| {
                    MobError::Internal(format!(
                        "failed to inspect persistent mob entry '{}': {error}",
                        path.display()
                    ))
                })?;
                if !file_type.is_file()
                    || path.extension().and_then(|ext| ext.to_str()) != Some("db")
                {
                    continue;
                }
                // Realm-scoped stores use reserved names. Only the prefixed
                // per-mob database namespace is eligible for restore or empty
                // sweep, so custody survives every individual mob lifecycle.
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(Self::MOB_STORE_FILE_PREFIX))
                {
                    continue;
                }
                // A store this state already registered (an earlier pass of
                // this scan was cancelled after its owned resume finished)
                // is the live mob: never resume the exact same store again.
                if self.registered_store(&path).await {
                    continue;
                }
                // The store of a mob this state destroyed is never resumed
                // while its cleanup is owed.
                if self.destroy_cleanup_owed_at(&path) {
                    continue;
                }

                let storage = self.bind_realm_profile_store(MobStorage::persistent(&path)?);
                if storage.is_event_log_empty().await? {
                    // A creation record that is not finalized names a mob
                    // whose log lost its MobCreated: the store disagrees with
                    // itself, so it is left unrestored as found, never
                    // swept. So is a store whose records disagree among
                    // themselves. With no record, or one a finished destroy
                    // finalized, the store is empty. Any other read fault
                    // fails the restore.
                    match storage.unfinalized_creation_mob().await {
                        Ok(Some(mob_id)) => {
                            self.keep_unrestored_mob(
                                Some(mob_id.clone()),
                                &path,
                                MobError::IncarnationInconsistent {
                                    mob_id,
                                    inconsistency:
                                        meerkat_mob::IncarnationInconsistency::RecordWithoutCreation,
                                },
                            )
                            .await?;
                            continue;
                        }
                        Ok(None) => {}
                        Err(MobError::IncarnationInconsistent {
                            mob_id,
                            inconsistency,
                        }) => {
                            self.keep_unrestored_mob(
                                Some(mob_id.clone()),
                                &path,
                                MobError::IncarnationInconsistent {
                                    mob_id,
                                    inconsistency,
                                },
                            )
                            .await?;
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                    if let Err(error) = Self::remove_storage_files(Some(path.as_path())).await {
                        tracing::error!(error = %error, "empty mob store sweep failed");
                    }
                    continue;
                }

                // Decide before any actor exists: a child whose owner is
                // archived is torn down without activation, and one whose
                // owner is unresolved is held, never resumed. The resume, the
                // teardown and the scavenge below are built in their own
                // frames: at opt-level 0 each awaited future reserved twice
                // its size in this poll frame, beneath every mob it restores.
                match self.child_mob_restore_gate(&storage, &path).await? {
                    ChildMobRestoreGate::Resume => {
                        // A failed resume is kept under the id the store
                        // names, read only then.
                        let kept = storage.clone();
                        let resumed = meerkat_runtime::stack_relief::box_in_own_frame(|| {
                            self.resume_persisted_mob(storage, &path)
                        })
                        .await;
                        if let Err(error) = resumed {
                            let mob_id = kept
                                .created_definition()
                                .await
                                .ok()
                                .flatten()
                                .map(|definition| definition.id);
                            self.keep_unrestored_mob(mob_id, &path, error).await?;
                        }
                    }
                    ChildMobRestoreGate::Teardown { mob_id, owner } => {
                        if let Err(error) = meerkat_runtime::stack_relief::box_in_own_frame(|| {
                            self.tear_down_archived_child_mob(storage, &path, mob_id.clone(), owner)
                        })
                        .await
                        {
                            tracing::warn!(
                                mob_id = %mob_id,
                                error = %error,
                                "restored child mob of an archived owner was not torn down"
                            );
                        }
                    }
                    ChildMobRestoreGate::Hold(held) => self.hold_child_mob(held).await,
                }
            }

            let _ = meerkat_runtime::stack_relief::box_in_own_frame(|| {
                self.scavenge_orphaned_bridge_session_scoped_mobs_inner()
            })
            .await;
            Ok(())
        }
    }

    /// Stand this state down: close its admission and wait for the work it
    /// owns, so a host can release what that work writes to (a durable
    /// child-mob root) in this process.
    ///
    /// The state stops admitting mob creates, inserts, restores, council
    /// runs and new periodic or convergent work, aborts its periodic and
    /// convergent tasks (sweeps, reconcilers, re-link passes; the next state
    /// over the same storage redoes them), and waits, until `deadline`, for
    /// every owned task (council runs, completion custodians, teardown
    /// handoffs, detached waits) and every admitted caller or detached
    /// holder (a create already past its admission, a live delegation
    /// delivery) to finish. Owned work admitted before the close may still
    /// start its second half while this runs; it is waited for too.
    /// [`MobStateStandDown::Clean`] then seals the state: it admits nothing
    /// more. The task set stays in the state, so a concurrent or cancelled
    /// call never reports clean while work runs, and a later call waits for
    /// whatever an unsettled one left.
    ///
    /// It does not shut down the mobs it manages. A host closes admission
    /// first ([`Self::close_admission`]), takes its snapshot of the mobs and
    /// shuts them down, then stands the state down, and finally looks again
    /// for a mob registered meanwhile by owned work (a child teardown): a
    /// sealed state adds none.
    pub async fn stand_down(&self, deadline: std::time::Instant) -> MobStateStandDown {
        let mut unsettled = self.close_admission_reasons(deadline).await;
        unsettled.extend(self.state_tasks.stand_down(deadline).await);
        if unsettled.is_empty() {
            MobStateStandDown::Clean
        } else {
            MobStateStandDown::Unsettled { reasons: unsettled }
        }
    }

    /// Close this state's admission and wait, until `deadline`, for the
    /// callers already admitted (a mob create, insert or restore, or a
    /// council admission, already past its check) to finish their writes;
    /// then close the admission of every mob the state manages
    /// (`MobHandle::close_admission` with
    /// [`meerkat_mob::MobAdmissionCloseReason::StandDown`]), so each mob's
    /// actor refuses new members, work, flows and resumes too.
    ///
    /// [`MobStateStandDown::Clean`] means no caller admitted before the close
    /// can still add a mob and every managed mob closed its own admission;
    /// owned work keeps running until [`Self::stand_down`], which also does
    /// all of this if it was not called. Idempotent.
    pub async fn close_admission(&self, deadline: std::time::Instant) -> MobStateStandDown {
        let unsettled = self.close_admission_reasons(deadline).await;
        if unsettled.is_empty() {
            MobStateStandDown::Clean
        } else {
            MobStateStandDown::Unsettled { reasons: unsettled }
        }
    }

    async fn close_admission_reasons(&self, deadline: std::time::Instant) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            // Admission cannot be fenced on wasm32: never report it closed.
            let _ = deadline;
            vec!["state admission cannot be closed on wasm32".to_string()]
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.close_admission_reasons_native(deadline).await
        }
    }

    /// Every wait below is bounded by the same `deadline`. A wait that
    /// misses it is reported and abandoned, never aborted on the other side:
    /// a close the actor received still takes effect, and a later call
    /// closes again (the close is idempotent).
    #[cfg(not(target_arch = "wasm32"))]
    async fn close_admission_reasons_native(&self, deadline: std::time::Instant) -> Vec<String> {
        let mut unsettled = self.state_tasks.close_admissions(deadline).await;
        let deadline = tokio::time::Instant::from_std(deadline);
        // The state admits no new mob now, and the callers admitted before
        // the close have inserted theirs (or are reported above): every mob
        // that can still take work is in this set.
        let Ok(mobs) = tokio::time::timeout_at(deadline, self.mobs.read()).await else {
            unsettled.push("managed mobs could not be read before the deadline".to_string());
            return unsettled;
        };
        let handles = mobs
            .iter()
            .map(|(mob_id, managed)| (mob_id.clone(), managed.handle.clone()))
            .collect::<Vec<_>>();
        drop(mobs);
        for (mob_id, handle) in handles {
            let Ok(closed) = tokio::time::timeout_at(
                deadline,
                handle.close_admission(meerkat_mob::MobAdmissionCloseReason::StandDown),
            )
            .await
            else {
                unsettled.push(format!(
                    "mob {mob_id}: admission close not answered before the deadline"
                ));
                continue;
            };
            match closed {
                Ok(busy) => {
                    if let Some(busy) = busy {
                        tracing::debug!(mob_id = %mob_id, ?busy, "mob closed its admission with work still in flight");
                    }
                }
                // A stopped actor admits nothing, and a shutting-down one
                // closed its admission before it began to drain.
                Err(
                    MobError::ActorCommandChannelClosed
                    | MobError::ActorReplyChannelClosed
                    | MobError::ShutdownInProgress { .. },
                ) => {}
                Err(error) => {
                    unsettled.push(format!("mob {mob_id}: admission not closed: {error}"));
                }
            }
        }
        unsettled
    }

    fn stood_down_error() -> MobError {
        MobError::Internal("this mob state has stood down and admits no new work".to_string())
    }

    /// Admit a caller that may add a mob, held until its insertion is
    /// done (see [`state_tasks::StateTasks::admit`]).
    fn admit(&self, name: &'static str) -> Result<state_tasks::StateHold, MobError> {
        self.state_tasks
            .admit(name)
            .ok_or_else(Self::stood_down_error)
    }

    /// Hold the state for the second half of admitted work, through its
    /// insertion (see [`state_tasks::StateTasks::hold`]).
    #[cfg(not(target_arch = "wasm32"))]
    fn hold(&self, name: &'static str) -> Result<state_tasks::StateHold, MobError> {
        self.state_tasks
            .hold(name)
            .ok_or_else(Self::stood_down_error)
    }

    /// Whether a managed mob was registered from exactly the store at
    /// `path`.
    #[cfg(not(target_arch = "wasm32"))]
    async fn registered_store(&self, path: &Path) -> bool {
        self.mobs
            .read()
            .await
            .values()
            .any(|managed| managed.storage_path.as_deref() == Some(path))
    }

    /// Resume one persisted mob store and register it as a managed mob.
    #[cfg(not(target_arch = "wasm32"))]
    async fn resume_persisted_mob(
        &self,
        storage: MobStorage,
        path: &Path,
    ) -> Result<MobId, MobError> {
        let hold = self.hold("mob resume")?;
        // Like a create, the resume runs to its registration as the state's
        // own task, so a dropped caller cannot leave a resumed actor outside
        // the managed set.
        // The resume is built in its own frame on either path: at opt-level 0
        // its future, moved by value into the state task or awaited inline,
        // reserved its size several times in this poll frame.
        if let Some(state) = self.self_weak.get().and_then(std::sync::Weak::upgrade) {
            let path = path.to_path_buf();
            return self
                .run_admitted_create(
                    "mob resume",
                    meerkat_runtime::stack_relief::box_in_own_frame(move || async move {
                        let _hold = hold;
                        state.resume_persisted_mob_held(storage, &path).await
                    }),
                )
                .await;
        }
        let _hold = hold;
        meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.resume_persisted_mob_held(storage, path)
        })
        .await
    }

    /// The body of [`Self::resume_persisted_mob`] under its hold.
    #[cfg(not(target_arch = "wasm32"))]
    async fn resume_persisted_mob_held(
        &self,
        storage: MobStorage,
        path: &Path,
    ) -> Result<MobId, MobError> {
        let scope = child_tool_policy::ChildMobScope::default();
        // The stored definition names the mob whose member tools this build
        // binds. It is read in its own frame, like the resume below.
        let created =
            meerkat_runtime::stack_relief::box_in_own_frame(|| storage.created_definition())
                .await?;
        #[cfg(test)]
        {
            let refused = self
                .refused_resume_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if refused.as_deref() == Some(path)
                && let Some(definition) = created.as_ref()
            {
                return Err(MobError::IncarnationInconsistent {
                    mob_id: definition.id.clone(),
                    inconsistency: meerkat_mob::IncarnationInconsistency::MultipleRecords {
                        count: 2,
                    },
                });
            }
        }
        let (member_external_tools, on_managed) = match created {
            Some(definition) => self.member_external_tools_for(&definition.id, None),
            None => (None, None),
        };
        // The builder and its resume are built in their own frame, and so is
        // the registration: at opt-level 0 each awaited future reserved twice
        // its size in this poll frame, beneath the mob's whole resume.
        let handle = meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.configure_builder(
                MobBuilder::for_resume(storage),
                scope.clone(),
                member_external_tools,
            )
            .before_activation(self.restore_scope_hook(scope))
            .resume()
        })
        .await?;
        #[cfg(test)]
        {
            let hold = self
                .resume_insert_hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
        }
        meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.register_resumed_mob(handle, path, on_managed)
        })
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn register_resumed_mob(
        &self,
        handle: MobHandle,
        path: &Path,
        on_managed: Option<MemberExternalToolsOnManaged>,
    ) -> Result<MobId, MobError> {
        let mob_id = handle.definition().id.clone();
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(_) if self.destroy_cleanup_owed(&mob_id) => {
                // A destroyed mob's storage cleanup is owed under this id;
                // that storage is never removed here.
                let _ = handle.destroy().await;
                return Err(Self::owed_destroy_cleanup_error(&mob_id));
            }
            Entry::Vacant(entry) => {
                entry.insert(self.managed_mob_entry(
                    handle,
                    Some(path.to_path_buf()),
                    ManagedInsertOrigin::RootRestore,
                ));
                // Under the managed set's write lock: a caller that finds
                // the mob also finds what its build bound.
                if let Some(on_managed) = on_managed {
                    on_managed(&mob_id);
                }
                self.note_mob_set_changed();
            }
            Entry::Occupied(_) => {
                let _ = handle.destroy().await;
                return Err(MobError::Internal(format!(
                    "duplicate persisted mob id during restore: {mob_id}"
                )));
            }
        }
        Ok(mob_id)
    }

    /// The restore gate of a persisted mob, decided from durable facts before
    /// any actor exists:
    /// - a mob that is not a child destroyed with its owner resumes as before;
    /// - a child whose history leaves a destroy to recover (an admitted
    ///   destroy or a durable Destroy quiesce) resumes into that recovery,
    ///   whatever the owner now reads as;
    /// - a child whose owner is committed resumes as before;
    /// - a child whose owner is archived is torn down without activation;
    /// - a child whose owner is absent or cannot be observed, or whose
    ///   history cannot be read, is held.
    #[cfg(not(target_arch = "wasm32"))]
    async fn child_mob_restore_gate(
        &self,
        storage: &MobStorage,
        path: &Path,
    ) -> Result<ChildMobRestoreGate, MobError> {
        let Some(definition) = storage.created_definition().await? else {
            return Ok(ChildMobRestoreGate::Resume);
        };
        let Some(owner) = storage.owner_bridge_binding().await? else {
            return Ok(ChildMobRestoreGate::Resume);
        };
        if !child_tool_policy::is_child_mob(owner.destroy_on_owner_archive) {
            return Ok(ChildMobRestoreGate::Resume);
        }
        match storage.destroy_recovery_pending().await {
            Ok(true) => return Ok(ChildMobRestoreGate::Resume),
            Ok(false) => {}
            Err(error) => {
                return Ok(ChildMobRestoreGate::Hold(HeldChildMob {
                    mob_id: definition.id,
                    storage_path: path.to_path_buf(),
                    owner: owner.bridge_session_id,
                    reason: ChildMobHoldReason::HistoryUnreadable(error.to_string()),
                    roster_read: Arc::default(),
                }));
            }
        }
        Ok(
            match self.resolve_child_mob_owner(&owner.bridge_session_id).await {
                Ok(false) => ChildMobRestoreGate::Resume,
                Ok(true) => ChildMobRestoreGate::Teardown {
                    mob_id: definition.id,
                    owner: owner.bridge_session_id,
                },
                Err(reason) => ChildMobRestoreGate::Hold(HeldChildMob {
                    mob_id: definition.id,
                    storage_path: path.to_path_buf(),
                    owner: owner.bridge_session_id,
                    reason,
                    roster_read: Arc::default(),
                }),
            },
        )
    }

    /// Resolve a child mob's owner: `Ok(false)` when it is committed,
    /// `Ok(true)` when it is confirmed archived, otherwise why it does not
    /// resolve.
    #[cfg(not(target_arch = "wasm32"))]
    async fn resolve_child_mob_owner(&self, owner: &SessionId) -> Result<bool, ChildMobHoldReason> {
        match self
            .session_service
            .observe_live_durable_source(owner)
            .await
        {
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed { .. }) => Ok(false),
            Ok(meerkat_mob::LiveDurableSourceObservation::Archived) => Ok(true),
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent) => {
                Err(ChildMobHoldReason::OwnerAbsent)
            }
            Ok(other) => Err(ChildMobHoldReason::OwnerUnobservable(format!("{other:?}"))),
            Err(error) => Err(ChildMobHoldReason::OwnerUnobservable(error.to_string())),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn hold_child_mob(&self, held: HeldChildMob) {
        tracing::warn!(
            mob_id = %held.mob_id,
            owner = %held.owner,
            reason = %held.reason,
            "holding a restored child mob before activation"
        );
        self.held_child_mobs
            .write()
            .await
            .insert(held.mob_id.clone(), held);
    }

    /// Keep a persisted mob whose resume failed unrestored, under the id its
    /// store names, so the rest of the restore goes on: it gets no actor, its
    /// store stays as found, and a lookup of the id answers the failure. A
    /// transport or persistence fault, which a later restore may get past,
    /// and a store that names no mob still fail the restore.
    #[cfg(not(target_arch = "wasm32"))]
    async fn keep_unrestored_mob(
        &self,
        mob_id: Option<MobId>,
        path: &Path,
        error: MobError,
    ) -> Result<(), MobError> {
        let Some(mob_id) = mob_id else {
            return Err(error);
        };
        if error.failure_class() == meerkat_mob::MobFailureClass::Transport {
            return Err(error);
        }
        tracing::warn!(
            mob_id = %mob_id,
            error = %error,
            "leaving a persisted mob unrestored: its resume failed"
        );
        self.unrestored_mobs.write().await.insert(
            path.to_path_buf(),
            UnrestoredMob {
                mob_id,
                cause: UnrestoredMobCause::of(&error),
            },
        );
        Ok(())
    }

    /// The persisted mob `mob_id` that was not restored, as its lookup
    /// answer.
    async fn unrestored_mob_error(&self, mob_id: &MobId) -> Option<MobError> {
        self.unrestored_mobs
            .read()
            .await
            .values()
            .find(|unrestored| &unrestored.mob_id == mob_id)
            .map(|unrestored| unrestored.cause.error(mob_id))
    }

    /// A member lookup of `session_id` over the persisted mob `mob_id` left
    /// unrestored with its store at `path`. That mob's incarnation is in
    /// doubt, so it is never a match: when its stored roster names the
    /// session, or cannot be read, the lookup refuses with
    /// [`MobError::ProvenanceUnavailable`] naming it. A roster that does not
    /// name the session leaves the lookup to the other mobs.
    #[cfg(not(target_arch = "wasm32"))]
    async fn unrestored_member_lookup(
        path: &Path,
        mob_id: &MobId,
        session_id: &SessionId,
    ) -> Result<(), MobError> {
        let unavailable = |error: Option<&dyn std::fmt::Display>| {
            match error {
                Some(error) => tracing::warn!(
                    mob_id = %mob_id,
                    %error,
                    "unrestored mob roster could not be read for a provenance lookup"
                ),
                None => tracing::warn!(
                    mob_id = %mob_id,
                    "unrestored mob roster names the session of a provenance lookup"
                ),
            }
            MobError::ProvenanceUnavailable {
                mob_id: Some(mob_id.clone()),
            }
        };
        let storage = MobStorage::persistent(path).map_err(|error| unavailable(Some(&error)))?;
        match storage.stored_member_of_session(session_id).await {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(unavailable(None)),
            Err(error) => Err(unavailable(Some(&error))),
        }
    }

    /// Tear down a persisted child mob without activating it: the actor
    /// admits the destroy (as the console principal) before any restored work
    /// starts, then the ordinary destroy envelope runs. A refused admission
    /// holds the child as `TeardownBlocked`; it admits no fresh destroy, but
    /// earlier recovery may have written, and an uncertain append error does
    /// not prove nothing was. A held child's row is replaced only once the
    /// resumed mob is registered as managed.
    #[cfg(not(target_arch = "wasm32"))]
    async fn tear_down_archived_child_mob(
        &self,
        storage: MobStorage,
        path: &Path,
        mob_id: MobId,
        owner: SessionId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        // The admission and the transfer to managed custody run detached
        // when this state is shared, so a cancelled caller cannot strand the
        // child between them.
        let custody = match self.self_weak.get().and_then(std::sync::Weak::upgrade) {
            Some(state) => {
                let path = path.to_path_buf();
                let mob_id = mob_id.clone();
                // The handoff, the direct admission and the destroy below are
                // built in their own frames: at opt-level 0 each future,
                // moved by value into the state task or awaited inline,
                // reserved its size several times in this poll frame.
                let handoff = self
                    .state_tasks
                    .spawn(
                        state_tasks::StateTaskKind::Owned,
                        "child mob teardown handoff",
                        meerkat_runtime::stack_relief::box_in_own_frame(move || async move {
                            state
                                .admit_child_teardown_into_custody(storage, path, mob_id, owner)
                                .await
                        }),
                    )
                    .ok_or_else(|| MobMcpDestroyError::Mob(Self::stood_down_error()))?;
                handoff.await.map_err(|_| {
                    MobMcpDestroyError::Mob(MobError::Internal(
                        "child mob teardown handoff ended without a result".to_string(),
                    ))
                })??
            }
            None => {
                meerkat_runtime::stack_relief::box_in_own_frame(|| {
                    self.admit_child_teardown_into_custody(
                        storage,
                        path.to_path_buf(),
                        mob_id.clone(),
                        owner,
                    )
                })
                .await?
            }
        };
        // While `custody` is held, this envelope is the only destroy in
        // flight; once it is dropped, on any path, the actor's own retry
        // recovers whatever cleanup remains.
        let result = meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.mob_destroy_unchecked_loaded(&mob_id, MobDestroyCaller::Console, None)
        })
        .await;
        drop(custody);
        result
    }

    /// Admit a held or archived child's destroy before it starts and move it
    /// into managed custody. Its held row is replaced only once the resumed
    /// mob is registered; a refusal keeps or makes it held.
    #[cfg(not(target_arch = "wasm32"))]
    async fn admit_child_teardown_into_custody(
        &self,
        storage: MobStorage,
        path: PathBuf,
        mob_id: MobId,
        owner: SessionId,
    ) -> Result<meerkat_mob::runtime::StartAdmittedDestroyCustody, MobMcpDestroyError> {
        // A teardown is still admitted while a stand-down runs; it is waited
        // for through the registration below.
        let _hold = self
            .hold("child teardown")
            .map_err(MobMcpDestroyError::Mob)?;
        let blocked = |error: &MobError| HeldChildMob {
            mob_id: mob_id.clone(),
            storage_path: path.clone(),
            owner: owner.clone(),
            reason: ChildMobHoldReason::TeardownBlocked(error.to_string()),
            roster_read: Arc::default(),
        };
        #[cfg(test)]
        {
            let hold = self
                .teardown_preflight_hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
        }
        if let Err(error) = self.workgraph_destroy_preflight(&mob_id).await {
            self.hold_child_mob(blocked(&error)).await;
            return Err(MobMcpDestroyError::Mob(error));
        }
        let scope = child_tool_policy::ChildMobScope::default();
        let (custody, custody_watch) = meerkat_mob::runtime::start_admitted_destroy_custody();
        let (member_external_tools, on_managed) = self.member_external_tools_for(&mob_id, None);
        // Built in its own frame, like the resume in
        // `resume_persisted_mob_held`.
        let resumed = meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.configure_builder(
                MobBuilder::for_resume(storage),
                scope.clone(),
                member_external_tools,
            )
            .before_activation(self.restore_scope_hook(scope))
            .admit_destroy_before_start(
                CommandAuthority::principal(self.console_principal.clone()),
                custody_watch,
            )
            .resume()
        })
        .await;
        let handle = match resumed {
            Ok(handle) => handle,
            Err(error) => {
                self.hold_child_mob(blocked(&error)).await;
                return Err(MobMcpDestroyError::Mob(error));
            }
        };
        meerkat_runtime::stack_relief::box_in_own_frame(|| {
            self.register_resumed_mob(handle, &path, on_managed)
        })
        .await
        .map_err(MobMcpDestroyError::Mob)?;
        // Managed custody now holds the child: only now does its hold end.
        self.held_child_mobs.write().await.remove(&mob_id);
        Ok(custody)
    }

    /// The held child mob `mob_id`, or the persisted mob `mob_id` that was not
    /// restored, as its operational lookup answer.
    async fn unmanaged_mob_error(&self, mob_id: &MobId) -> Option<MobError> {
        if let Some(error) = self.unrestored_mob_error(mob_id).await {
            return Some(error);
        }
        self.held_child_mobs.read().await.get(mob_id).map(|held| {
            MobError::held_before_activation(&held.mob_id, &held.owner, held.reason.token())
        })
    }

    /// The restored child mobs held before activation, with their owners.
    pub async fn held_child_mobs(&self) -> Vec<(MobId, SessionId)> {
        if !self.ensure_restored_best_effort("held_child_mobs").await {
            return Vec::new();
        }
        self.held_child_mobs
            .read()
            .await
            .values()
            .map(|held| (held.mob_id.clone(), held.owner.clone()))
            .collect()
    }

    /// The persisted mobs whose resume failed at restore, each with the typed
    /// error a lookup of its id answers. They are not in [`Self::mob_list`];
    /// their stores are kept as found.
    pub async fn unrestored_mobs(&self) -> Vec<(MobId, MobError)> {
        if !self.ensure_restored_best_effort("unrestored_mobs").await {
            return Vec::new();
        }
        self.unrestored_mobs
            .read()
            .await
            .values()
            .map(|unrestored| {
                (
                    unrestored.mob_id.clone(),
                    unrestored.cause.error(&unrestored.mob_id),
                )
            })
            .collect()
    }

    /// Operator release of a held child mob whose owner never resolves: tear
    /// it down without activation, through the same admitted destroy as an
    /// archived owner. An explicit operator decision; restore never does
    /// this on its own.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn destroy_held_child_mob(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        self.ensure_restored()
            .await
            .map_err(MobMcpDestroyError::Mob)?;
        let _transfer = self.held_child_transfer.lock().await;
        // The held row stays until the teardown registers the child as
        // managed: a cancelled or failed release leaves it held.
        let held = self
            .held_child_mobs
            .read()
            .await
            .get(mob_id)
            .cloned()
            .ok_or_else(|| MobMcpDestroyError::Mob(MobError::MobNotFound(mob_id.clone())))?;
        let storage = self.bind_realm_profile_store(
            MobStorage::persistent(&held.storage_path).map_err(MobMcpDestroyError::Mob)?,
        );
        self.tear_down_archived_child_mob(
            storage,
            &held.storage_path,
            held.mob_id.clone(),
            held.owner.clone(),
        )
        .await
    }

    /// Re-observe every held child mob: a destroy to recover in its history
    /// or a committed owner resumes it, an archived owner tears it down without
    /// activation, anything else keeps it held.
    #[cfg(not(target_arch = "wasm32"))]
    async fn reobserve_held_child_mobs(&self) -> Vec<MobId> {
        // A reobservation resumes or tears down held children into the
        // managed set: a closed state starts none, and one already admitted
        // is waited for by the close.
        let Ok(_admission) = self.admit("held child reobservation") else {
            return Vec::new();
        };
        let _transfer = self.held_child_transfer.lock().await;
        let held = self
            .held_child_mobs
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut destroyed = Vec::new();
        for held in held {
            // A transfer cancelled after registration leaves both rows: the
            // managed mob owns the child, so only the stale hold is dropped.
            if self.mobs.read().await.contains_key(&held.mob_id) {
                self.held_child_mobs.write().await.remove(&held.mob_id);
                continue;
            }
            let storage = match MobStorage::persistent(&held.storage_path) {
                Ok(storage) => self.bind_realm_profile_store(storage),
                Err(error) => {
                    tracing::warn!(mob_id = %held.mob_id, error = %error, "held child mob store could not be reopened");
                    continue;
                }
            };
            // A store whose destroy already finished elsewhere is swept like
            // restore sweeps an empty store.
            if matches!(storage.is_event_log_empty().await, Ok(true)) {
                drop(storage);
                self.held_child_mobs.write().await.remove(&held.mob_id);
                if let Err(error) =
                    Self::remove_storage_files(Some(held.storage_path.as_path())).await
                {
                    tracing::error!(error = %error, "empty held child mob store sweep failed");
                }
                destroyed.push(held.mob_id.clone());
                continue;
            }
            let archived = match storage.destroy_recovery_pending().await {
                Ok(true) => false,
                Ok(false) => match self.resolve_child_mob_owner(&held.owner).await {
                    Ok(archived) => archived,
                    Err(reason) => {
                        if let Some(entry) =
                            self.held_child_mobs.write().await.get_mut(&held.mob_id)
                        {
                            entry.reason = reason;
                        }
                        continue;
                    }
                },
                Err(error) => {
                    if let Some(entry) = self.held_child_mobs.write().await.get_mut(&held.mob_id) {
                        entry.reason = ChildMobHoldReason::HistoryUnreadable(error.to_string());
                    }
                    continue;
                }
            };
            // The held row stays until the child is registered as managed.
            // The teardown and the resume are built in their own frames: at
            // opt-level 0 each awaited future reserved twice its size in
            // this poll frame.
            if archived {
                if meerkat_runtime::stack_relief::box_in_own_frame(|| {
                    self.tear_down_archived_child_mob(
                        storage,
                        &held.storage_path,
                        held.mob_id.clone(),
                        held.owner.clone(),
                    )
                })
                .await
                .is_ok()
                {
                    destroyed.push(held.mob_id.clone());
                }
            } else {
                match meerkat_runtime::stack_relief::box_in_own_frame(|| {
                    self.resume_persisted_mob(storage, &held.storage_path)
                })
                .await
                {
                    Ok(_) => {
                        self.held_child_mobs.write().await.remove(&held.mob_id);
                    }
                    Err(error) => {
                        tracing::warn!(mob_id = %held.mob_id, error = %error, "held child mob could not be resumed");
                    }
                }
            }
        }
        destroyed
    }

    async fn ensure_restored(&self) -> Result<(), MobError> {
        if self.persistent_storage_root.is_none() {
            // A host may still supply a durable council store without a
            // persistent root (Meta does): its councils need the same
            // post-restart convergence.
            self.schedule_temporary_council_recovery();
            self.schedule_local_forked_participant_sweeper();
            return Ok(());
        }
        if let Some(error) = &self.persistent_storage_setup_error {
            return Err(MobError::Internal(format!(
                "persistent mob state setup failed: {error}"
            )));
        }
        {
            let mut restored = self.restore_lock.lock().await;
            if !*restored {
                // A restore registers mobs: held until it is done.
                let _admission = self.admit("restore")?;
                // Built in its own frame: every caller of `ensure_restored`
                // (each mob verb) would otherwise reserve the restore
                // future's debug frame even on the already-restored path.
                meerkat_runtime::stack_relief::box_in_own_frame(|| {
                    self.restore_from_persistent_storage()
                })
                .await?;
                *restored = true;
                if self.persistent_storage_root.is_some() {
                    self.mob_set_restored
                        .store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
        // The restore lock is released BEFORE scheduling, and the sweep runs
        // on its own task, so recovery can use the ordinary mob verbs (which
        // call back into `ensure_restored`) without recursion or deadlock.
        self.schedule_temporary_council_recovery();
        self.schedule_fork_relink();
        self.schedule_local_forked_participant_sweeper();
        Ok(())
    }

    /// Schedule the one-time post-restore fork_off re-link pass (see
    /// [`crate::fork_relink`]). Runs on its own task, like council recovery.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn schedule_fork_relink(&self) {
        use std::sync::atomic::Ordering;
        if self.fork_relink_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(weak) = self.self_weak.get().cloned() else {
            self.fork_relink_scheduled.store(false, Ordering::SeqCst);
            return;
        };
        let restored_before_ms = self.created_at_ms;
        let _ = self.state_tasks.spawn(
            state_tasks::StateTaskKind::Convergent,
            "fork_off re-link pass",
            meerkat_runtime::stack_relief::box_in_own_frame(move || async move {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let reports = crate::fork_relink::relink_restored_fork_children(
                    &state,
                    restored_before_ms,
                    true,
                )
                .await;
                crate::fork_relink::log_failed_child_retirements(&reports);
                if !reports.is_empty() {
                    tracing::info!(
                        children = reports.len(),
                        "fork_off re-link pass handled children from a previous process"
                    );
                }
            }),
        );
    }

    async fn ensure_restored_best_effort(&self, action: &str) -> bool {
        match self.ensure_restored().await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(action, error = %error, "mob state restore failed");
                false
            }
        }
    }

    /// Access the underlying session service.
    pub fn session_service(&self) -> Arc<dyn MobSessionService> {
        self.session_service.clone()
    }

    pub async fn mob_create_definition(
        &self,
        definition: MobDefinition,
    ) -> Result<MobId, MobError> {
        self.mob_create_definition_inner(definition, None, None, None)
            .await
            .map(|created| created.mob_id)
    }

    /// [`Self::mob_create_definition`] whose members build with
    /// `member_external_tools` as their default external tools, in place of
    /// [`Self::with_external_tools_provider`]'s state-wide source: the tools
    /// of the caller that creates the mob, such as its connection's callback
    /// tools. They belong to this mob alone; a mob later created under the
    /// same id, or restored, never receives them.
    pub async fn mob_create_definition_with_member_external_tools(
        &self,
        definition: MobDefinition,
        member_external_tools: meerkat_mob::ExternalToolsProvider,
    ) -> Result<MobId, MobError> {
        self.mob_create_definition_inner(definition, None, Some(member_external_tools), None)
            .await
            .map(|created| created.mob_id)
    }

    /// [`Self::mob_create_definition`] for a creator that will hold the
    /// mob's callback route: the created mob keeps `claim_verifier`, the
    /// verifier of the claim token its creator receives, in its own store
    /// before it is managed, so [`Self::mob_claim_member_external_tools`]
    /// needs that token for it. `member_external_tools`, when given, are the
    /// creator's own, as for
    /// [`Self::mob_create_definition_with_member_external_tools`].
    pub async fn mob_create_definition_with_claim_verifier(
        &self,
        definition: MobDefinition,
        member_external_tools: Option<meerkat_mob::ExternalToolsProvider>,
        claim_verifier: meerkat_mob::MobClaimVerifier,
    ) -> Result<MobId, MobError> {
        self.mob_create_definition_inner(
            definition,
            None,
            member_external_tools,
            Some(claim_verifier),
        )
        .await
        .map(|created| created.mob_id)
    }

    /// Claim the member external tools of an existing mob for a caller (an
    /// RPC connection claiming a mob's callback route) that presents the
    /// mob's claim token.
    ///
    /// Needs the console owner (`SendCommand`) and is held by this state's
    /// admission while it runs, so a stood-down state admits no claim; the
    /// host's create hook is not asked, since a claim creates nothing. A
    /// restored mob is restored first, so the claim reaches the tools its
    /// restored members were built with. The token is checked against the
    /// verifier the mob keeps while the managed set is read with the mob in
    /// it; a token that is not the mob's, or a mob that keeps no verifier,
    /// refuses the claim ([`CallbackRouteClaimTokenRefused`]) before `claim`
    /// runs. `claim` is the caller's own binding of what it claims; it runs
    /// under the same read (so after the mob's
    /// [`MemberExternalTools::on_managed`]), and its answer is returned
    /// unchanged.
    pub async fn mob_claim_member_external_tools<T, F>(
        &self,
        mob_id: &MobId,
        claim_token: &meerkat_mob::MobClaimToken,
        claim: F,
    ) -> Result<Result<T, CallbackRouteClaimTokenRefused>, MobError>
    where
        F: FnOnce() -> T + Send,
        T: Send,
    {
        self.require_console_owner(ControlScope::SendCommand)?;
        let _admission = self.admit("mob callback route claim")?;
        self.ensure_restored().await?;
        let mobs = self.mobs.read().await;
        let Some(managed) = mobs.get(mob_id) else {
            drop(mobs);
            return Err(self
                .unmanaged_mob_error(mob_id)
                .await
                .unwrap_or_else(|| MobError::MobNotFound(mob_id.clone())));
        };
        let answer = match managed
            .handle
            .check_callback_claim_token(claim_token)
            .await?
        {
            meerkat_mob::CallbackClaimCheck::Verified => Ok(claim()),
            meerkat_mob::CallbackClaimCheck::Mismatch => {
                Err(CallbackRouteClaimTokenRefused::Mismatch)
            }
            meerkat_mob::CallbackClaimCheck::NotClaimable => {
                Err(CallbackRouteClaimTokenRefused::NotClaimable)
            }
        };
        drop(mobs);
        Ok(answer)
    }

    /// Mint a callback claim token for a managed mob that keeps no verifier
    /// (one created before 0.9.3, or other than over RPC), for the host to
    /// hand to the one connection it trusts with the mob's callback route.
    /// A Rust API only: no wire method reaches it. `None` when the mob
    /// already keeps a verifier, which is never replaced.
    ///
    /// Needs the console owner (`SendCommand`) and this state's admission,
    /// as a claim does.
    pub async fn mint_callback_claim_token(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<meerkat_mob::MobClaimToken>, MobError> {
        self.require_console_owner(ControlScope::SendCommand)?;
        let _admission = self.admit("mob callback claim token mint")?;
        self.ensure_restored().await?;
        let mobs = self.mobs.read().await;
        let Some(managed) = mobs.get(mob_id) else {
            drop(mobs);
            return Err(self
                .unmanaged_mob_error(mob_id)
                .await
                .unwrap_or_else(|| MobError::MobNotFound(mob_id.clone())));
        };
        let token = meerkat_mob::MobClaimToken::mint()
            .map_err(|error| MobError::Internal(error.to_string()))?;
        let recorded = managed
            .handle
            .record_callback_claim_verifier_if_absent(&token.verifier())
            .await?;
        drop(mobs);
        Ok(recorded.then_some(token))
    }

    pub async fn mob_create_from_mobpack(
        &self,
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        source_identity: meerkat_mob::MobDefinitionSourceIdentity,
    ) -> Result<MobId, MobError> {
        // (b) gate, ADJ-P5-11 (same class as mob_create_definition).
        self.require_console_owner(ControlScope::SendCommand)?;
        // A mobpack create is not a child mob's: it offers no tool bundle.
        self.refuse_unoffered_tool_bundles(&definition, false)?;
        let admission = self.admit("mobpack create")?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(state) = self.self_weak.get().and_then(std::sync::Weak::upgrade) {
            return self
                .run_admitted_create("mobpack create", async move {
                    let _admission = admission;
                    state
                        .mob_create_from_mobpack_admitted(
                            definition,
                            packed_skills,
                            source_identity,
                        )
                        .await
                })
                .await;
        }
        let _admission = admission;
        self.mob_create_from_mobpack_admitted(definition, packed_skills, source_identity)
            .await
    }

    /// The body of [`Self::mob_create_from_mobpack`] after its admission,
    /// through the insertion of the created mob.
    async fn mob_create_from_mobpack_admitted(
        &self,
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        source_identity: meerkat_mob::MobDefinitionSourceIdentity,
    ) -> Result<MobId, MobError> {
        let mut definition = MobBuilder::lower_mobpack_definition(definition, &packed_skills)?;
        definition.source_identity = Some(source_identity);
        let mob_id = definition.id.clone();
        self.ensure_restored().await?;
        if let Some(existing) = self.mobs.read().await.get(&mob_id) {
            let existing_definition = existing.handle.definition();
            if existing_definition.source_identity == definition.source_identity
                && existing_definition == &definition
            {
                return Ok(mob_id);
            }
            return Err(MobError::Internal(format!(
                "duplicate mobpack create for mob id '{mob_id}' does not match existing verified pack identity"
            )));
        }
        if self.destroy_cleanup_owed(&mob_id) {
            return Err(Self::owed_destroy_cleanup_error(&mob_id));
        }
        // The host admits or refuses the create before anything of the mob
        // exists. A mobpack is the host's own create: no owner session.
        self.admit_create(MobCreateRequest {
            mob_id: mob_id.clone(),
            owner_bridge_session_id: None,
            implicit_delegation_mob: false,
        })
        .await?;
        let (storage, storage_path) = self.storage_for_new_mob(&mob_id).await?;
        let (member_external_tools, on_managed) = self.member_external_tools_for(&mob_id, None);
        let handle = self
            .configure_builder(
                MobBuilder::new(definition, storage),
                child_tool_policy::ChildMobScope::default(),
                member_external_tools,
            )
            .with_provenance(meerkat_mob::MobProvenance::Host)
            .create()
            .await?;
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(_) if self.destroy_cleanup_owed(&mob_id) => {
                // A destroyed mob's storage cleanup is owed under this id;
                // that storage is never removed here.
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "mobpack create refused under an owed destroy cleanup failed"
                    );
                }
                Err(Self::owed_destroy_cleanup_error(&mob_id))
            }
            Entry::Vacant(entry) => {
                entry.insert(self.managed_mob_entry(
                    handle,
                    storage_path,
                    ManagedInsertOrigin::OwnCreate,
                ));
                if let Some(on_managed) = on_managed {
                    on_managed(&mob_id);
                }
                self.note_mob_set_changed();
                Ok(mob_id)
            }
            Entry::Occupied(_) => {
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "duplicate mobpack create cleanup failed"
                    );
                }
                if let Err(error) = Self::remove_storage_files(storage_path.as_deref()).await {
                    tracing::error!(error = %error, "duplicate mobpack storage cleanup failed");
                }
                Err(MobError::Internal(format!(
                    "duplicate mobpack create raced for mob id '{mob_id}' before verified identity could be committed"
                )))
            }
        }
    }

    /// Create `definition` for the caller session `owner_bridge_session_id`,
    /// as the host. The session owns the mob, and its creation record names
    /// it as `MobProvenance::Session`, or as the member's `owner_session` in
    /// `MobProvenance::Member`, but the create grants the session nothing:
    /// after a restart the agent mob tools do not re-derive a manage grant
    /// for it (`MobMcpState::session_is_recorded_creator`). Only a session's
    /// own create with its create capability (the agent mob tools'
    /// `mob_create`) makes it the creator whose grant is re-derived.
    #[doc(hidden)]
    pub async fn mob_create_definition_with_owner_bridge_session(
        &self,
        definition: MobDefinition,
        owner_bridge_session_id: SessionId,
        destroy_on_owner_archive: bool,
        implicit_delegation_mob: bool,
    ) -> Result<MobId, MobError> {
        self.create_mob_for_owner_bridge_session(
            definition,
            owner_bridge_session_id,
            destroy_on_owner_archive,
            implicit_delegation_mob,
            SessionCreateCaller::Host,
        )
        .await
        .map(|created| created.mob_id)
    }

    /// [`Self::mob_create_definition_with_owner_bridge_session`] made by
    /// `caller`, answering the incarnation of the mob this create built: the
    /// only incarnation a mob operator grant for the create may name.
    pub(crate) async fn create_mob_for_owner_bridge_session(
        &self,
        definition: MobDefinition,
        owner_bridge_session_id: SessionId,
        destroy_on_owner_archive: bool,
        implicit_delegation_mob: bool,
        caller: SessionCreateCaller,
    ) -> Result<CreatedMob, MobError> {
        self.mob_create_definition_inner(
            definition,
            Some((
                owner_bridge_session_id,
                destroy_on_owner_archive,
                implicit_delegation_mob,
                caller,
            )),
            None,
            None,
        )
        .await
    }

    async fn mob_create_definition_inner(
        &self,
        definition: MobDefinition,
        owner_bridge_session_authority: Option<(SessionId, bool, bool, SessionCreateCaller)>,
        member_external_tools: Option<meerkat_mob::ExternalToolsProvider>,
        claim_verifier: Option<meerkat_mob::MobClaimVerifier>,
    ) -> Result<CreatedMob, MobError> {
        // (b) gate, ADJ-P5-11: no per-mob grant can authorize creating a
        // mob that does not exist yet.
        self.require_console_owner(ControlScope::SendCommand)?;
        // Before anything of the mob exists: an opt-in names only what this
        // create offers.
        let child = owner_bridge_session_authority.as_ref().is_some_and(
            |(_, destroy_on_owner_archive, _, _)| {
                child_tool_policy::is_child_mob(*destroy_on_owner_archive)
            },
        );
        self.refuse_unoffered_tool_bundles(&definition, child)?;
        let admission = self.admit("mob create")?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(state) = self.self_weak.get().and_then(std::sync::Weak::upgrade) {
            return self
                .run_admitted_create("mob create", async move {
                    let _admission = admission;
                    state
                        .mob_create_definition_admitted(
                            definition,
                            owner_bridge_session_authority,
                            member_external_tools,
                            claim_verifier,
                        )
                        .await
                })
                .await;
        }
        let _admission = admission;
        self.mob_create_definition_admitted(
            definition,
            owner_bridge_session_authority,
            member_external_tools,
            claim_verifier,
        )
        .await
    }

    /// Run an admitted create, which ends by inserting the mob it built, as
    /// a task this state owns, and wait for its answer. A caller dropped
    /// mid-create then cannot leave a created mob outside the managed set,
    /// where no stand-down would see it: the task finishes the insertion (or
    /// cleans up a duplicate), and a stand-down waits for it.
    #[cfg(not(target_arch = "wasm32"))]
    async fn run_admitted_create<F, T>(&self, name: &'static str, create: F) -> Result<T, MobError>
    where
        F: std::future::Future<Output = Result<T, MobError>> + Send + 'static,
        T: Send + 'static,
    {
        let answer = self
            .state_tasks
            .spawn(state_tasks::StateTaskKind::Owned, name, create)
            .ok_or_else(Self::stood_down_error)?;
        answer.await.map_err(|_| {
            MobError::Internal(format!("{name} ended without an answer (it panicked)"))
        })?
    }

    /// The body of [`Self::mob_create_definition_inner`] after its
    /// admission, through the insertion of the created mob.
    async fn mob_create_definition_admitted(
        &self,
        mut definition: MobDefinition,
        owner_bridge_session_authority: Option<(SessionId, bool, bool, SessionCreateCaller)>,
        member_external_tools: Option<meerkat_mob::ExternalToolsProvider>,
        claim_verifier: Option<meerkat_mob::MobClaimVerifier>,
    ) -> Result<CreatedMob, MobError> {
        self.hydrate_definition_skill_sources(&mut definition)
            .await?;
        let mob_id = definition.id.clone();
        self.ensure_restored().await?;
        if self.mobs.read().await.contains_key(&mob_id) {
            return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
        }
        // A store left unrestored is kept as found: no create takes its id.
        if let Some(error) = self.unrestored_mob_error(&mob_id).await {
            return Err(error);
        }
        if self.destroy_cleanup_owed(&mob_id) {
            return Err(Self::owed_destroy_cleanup_error(&mob_id));
        }
        // Who creates the mob, recorded with its creation: the host when no
        // session calls, otherwise what the complete lookup of the hosted
        // rosters finds for the caller session, and whether that session
        // makes the create with its own create capability. A lookup that
        // cannot be complete refuses the create before anything of the mob
        // exists.
        let provenance = match owner_bridge_session_authority.as_ref() {
            None => meerkat_mob::MobProvenance::Host,
            Some((owner_bridge_session_id, _, _, caller)) => session_creation_provenance(
                owner_bridge_session_id,
                self.session_member_matches(owner_bridge_session_id).await?,
                *caller,
            ),
        };
        // The host admits or refuses the create before anything of the mob
        // exists, told what this path knows: the owner session and whether
        // this is that session's implicit delegation mob. Only the implicit
        // path sets the flag, and only under the id it derives from the
        // owner session does it count.
        self.admit_create(MobCreateRequest {
            mob_id: mob_id.clone(),
            owner_bridge_session_id: owner_bridge_session_authority
                .as_ref()
                .map(|(owner_bridge_session_id, _, _, _)| owner_bridge_session_id.clone()),
            implicit_delegation_mob: owner_bridge_session_authority.as_ref().is_some_and(
                |(owner_bridge_session_id, _, implicit_delegation_mob, _)| {
                    *implicit_delegation_mob
                        && MobDefinition::implicit(&owner_bridge_session_id.to_string(), "").id
                            == mob_id
                },
            ),
        })
        .await?;
        let (storage, storage_path) = self.storage_for_new_mob(&mob_id).await?;
        let child = owner_bridge_session_authority.as_ref().is_some_and(
            |(_, destroy_on_owner_archive, _, _)| {
                child_tool_policy::is_child_mob(*destroy_on_owner_archive)
            },
        );
        if child {
            // Host tool bundles reach a child mob only through the host.
            self.child_tool_bundles.supply(&mut definition);
            if let Some(bundles) = self.additional_child_tool_bundles.get() {
                bundles.supply(&mut definition);
            }
            self.child_mcp_servers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .supply(&mut definition)?;
        }
        let scope = child_tool_policy::ChildMobScope::new(child);
        let (member_external_tools, on_managed) =
            self.member_external_tools_for(&mob_id, member_external_tools);
        let mut builder = self.configure_builder(
            MobBuilder::new(definition.clone(), storage),
            scope,
            member_external_tools,
        );
        if let Some((
            owner_bridge_session_id,
            destroy_on_owner_archive,
            implicit_delegation_mob,
            _,
        )) = owner_bridge_session_authority
        {
            builder = builder.with_owner_bridge_session_create_authority(
                owner_bridge_session_id,
                destroy_on_owner_archive,
                implicit_delegation_mob,
            );
        }
        if let Some(claim_verifier) = claim_verifier {
            builder = builder.with_callback_claim_verifier(claim_verifier);
        }
        let handle = builder.with_provenance(provenance).create().await?;
        let incarnation = handle.live_incarnation();
        #[cfg(test)]
        {
            let hold = self
                .create_insert_hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
        }
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(_) if self.destroy_cleanup_owed(&mob_id) => {
                // A destroyed mob's storage cleanup is owed under this id;
                // that storage is never removed here.
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "mob create refused under an owed destroy cleanup failed"
                    );
                }
                return Err(Self::owed_destroy_cleanup_error(&mob_id));
            }
            Entry::Vacant(entry) => {
                entry.insert(self.managed_mob_entry(
                    handle,
                    storage_path,
                    ManagedInsertOrigin::OwnCreate,
                ));
                if let Some(on_managed) = on_managed {
                    on_managed(&mob_id);
                }
                self.note_mob_set_changed();
            }
            Entry::Occupied(_) => {
                // Race-safe duplicate guard: avoid leaking the just-created runtime.
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "duplicate mob create cleanup failed"
                    );
                }
                if let Err(error) = Self::remove_storage_files(storage_path.as_deref()).await {
                    tracing::error!(error = %error, "duplicate mob create storage cleanup failed");
                }
                return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
            }
        }
        Ok(CreatedMob {
            mob_id,
            incarnation,
        })
    }

    /// Register an existing foreign mob handle in this dispatcher state.
    ///
    /// The handle is not rebuilt and therefore does not participate in this
    /// state's shared capability custody. Hosts needing shared custody must
    /// compose it with [`MobStorage::with_forked_participant_store`] before
    /// building the handle.
    ///
    /// A state that has stood down (or is standing down) refuses the handle
    /// and logs it; [`Self::try_mob_insert_handle`] returns the refusal.
    pub async fn mob_insert_handle(&self, mob_id: MobId, handle: MobHandle) {
        if let Err(error) = self.try_mob_insert_handle(mob_id.clone(), handle).await {
            tracing::error!(mob_id = %mob_id, %error, "mob handle not inserted");
        }
    }

    /// [`Self::mob_insert_handle`], refusing with a typed error once a
    /// stand-down has started.
    ///
    /// Insert a mob's handle before its members can run turns: a member
    /// lookup ([`Self::session_member_matches`]) reads the rosters of the
    /// mobs hosted when it runs, so a member of a mob not inserted yet is not
    /// found, and a create by its session records `Session`.
    pub async fn try_mob_insert_handle(
        &self,
        mob_id: MobId,
        handle: MobHandle,
    ) -> Result<(), MobError> {
        let _admission = self.admit("mob insert")?;
        {
            let mut mobs = self.mobs.write().await;
            if self.destroy_cleanup_owed(&mob_id) {
                return Err(Self::owed_destroy_cleanup_error(&mob_id));
            }
            // The entry latches before the handle is visible: a member
            // lookup that could see the handle reads the latch set.
            mobs.insert(
                mob_id.clone(),
                self.managed_mob_entry(handle.clone(), None, ManagedInsertOrigin::HostInsert),
            );
        }
        self.note_mob_set_changed();
        // A host that restores mobs by inserting their handles (Meta) gets
        // the council sweep (when its council store is durable) and the
        // fork_off re-link for each restored mob here.
        self.schedule_temporary_council_recovery();
        if self.claim_fork_relink(&mob_id) {
            let service = self.session_service.clone();
            let delivery = crate::fork_relink::RelinkDelivery::from_state(self);
            let restored_before_ms = self.created_at_ms;
            let _ = self.state_tasks.spawn(
                state_tasks::StateTaskKind::Convergent,
                "inserted mob fork_off re-link",
                async move {
                    // A handle inserted before its mob runs (Meta restores a
                    // stopped mob and activates it later) cannot serve a forker
                    // yet: its outcome is durable and applied once it can.
                    let reports = crate::fork_relink::relink_mob_fork_children(
                        service,
                        delivery,
                        &mob_id,
                        &handle,
                        restored_before_ms,
                    )
                    .await;
                    crate::fork_relink::log_failed_child_retirements(&reports);
                    if !reports.is_empty() {
                        tracing::info!(
                            mob_id = %mob_id,
                            children = reports.len(),
                            "fork_off re-link handled children from a previous process"
                        );
                    }
                },
            );
        }
        Ok(())
    }

    /// The mobs this state manages, as a live view the re-link reads afresh
    /// each time it looks up a job's owner (a mob inserted later is found).
    pub(crate) fn managed_mobs(&self) -> crate::fork_relink::ManagedMobs {
        crate::fork_relink::ManagedMobs::new(Arc::clone(&self.mobs), self.console_principal.clone())
    }

    pub(crate) fn runtime_adapter_for_relink(
        &self,
    ) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        self.runtime_adapter.clone()
    }

    /// Claim the one-time fork_off re-link of `mob_id` for this state.
    pub(crate) fn claim_fork_relink(&self, mob_id: &MobId) -> bool {
        self.fork_relinked_mobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(mob_id.clone())
    }

    /// Run the fork_off re-link for every managed mob now and report what it
    /// did. Hosts normally get this automatically after restore; this is the
    /// explicit entry point (and the test seam).
    pub async fn relink_restored_fork_children(
        self: &Arc<Self>,
    ) -> Vec<crate::fork_relink::ForkRelinkReport> {
        // Explicit runs ignore the automatic pass's claims; delivery is
        // idempotent per job, so running twice never records twice.
        crate::fork_relink::relink_restored_fork_children(self, self.created_at_ms, false).await
    }

    /// Deliver the outcome of every detached council from an earlier process
    /// whose convener is still owed it, and report what was done. Persistent
    /// restoration runs this automatically after the council recovery sweep;
    /// a host that shares its state with `Arc::new` and recovers councils by
    /// calling [`TemporaryCouncilCoordinator::recover_unfinished`] calls this
    /// after it. Delivery is idempotent per job.
    pub async fn relink_detached_councils(
        self: &Arc<Self>,
    ) -> Vec<crate::council_relink::CouncilRelinkReport> {
        crate::council_relink::relink_detached_councils(self, self.created_at_ms).await
    }

    /// Change signal over the managed-mob handle set: the receiver wakes when
    /// a mob is registered or removed. Pair with per-handle
    /// [`meerkat_mob::MobMachineStateChanges`] to observe the full mob tree
    /// without interval polling.
    pub fn mob_set_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.mob_set_epoch.subscribe()
    }

    fn note_mob_set_changed(&self) {
        self.mob_set_epoch.send_modify(|epoch| *epoch += 1);
    }

    /// Return known mob handles without asking each mob actor for live status.
    ///
    /// Observation surfaces use this while child mobs are actively processing:
    /// a status query would queue behind the same actor work the UI is trying
    /// to observe.
    pub async fn mob_handles_snapshot(&self) -> Result<Vec<(MobId, MobHandle)>, MobError> {
        // (b) gate, ADJ-P5-11: cross-mob enumeration is Owner-only in v1 —
        // a per-mob List grant cannot authorize it (this is also mob_list's
        // gate; the raw-handle snapshot must not be a graded sibling).
        self.require_console_owner(ControlScope::List)?;
        self.ensure_restored().await?;
        // A destroyed mob (its phase flips when the destroy is admitted) is
        // not handed out: a clone taken now would keep observing the
        // database its destroy's storage cleanup drains. A mob whose actor
        // has stopped keeps its last live phase and is handed out: hiding it
        // would make its failure silently absent from the surfaces built on
        // this snapshot, which read why from the handle
        // (`lifecycle_observation`, `status`).
        Ok(self
            .mobs
            .read()
            .await
            .iter()
            .filter(|(_, managed)| {
                managed.handle.status_observation_snapshot() != MobState::Destroyed
            })
            .map(|(id, managed)| (id.clone(), managed.handle.clone()))
            .collect())
    }

    /// Every managed mob, with its lifecycle as observed without asking its
    /// actor (see [`meerkat_mob::MobLifecycleObservation`]).
    pub async fn mob_list(
        &self,
    ) -> Result<Vec<(MobId, meerkat_mob::MobLifecycleObservation)>, MobError> {
        // The gate of `mob_handles_snapshot` (cross-mob enumeration is
        // Owner-only), but every managed mob is listed: a destroyed mob still
        // managed, such as the retry anchor of a destroy whose native part
        // is incomplete, shows as `Destroyed`. Only its handle is withheld
        // from snapshots.
        self.require_console_owner(ControlScope::List)?;
        self.ensure_restored().await?;
        Ok(self
            .mobs
            .read()
            .await
            .iter()
            .map(|(id, managed)| (id.clone(), managed.handle.lifecycle_observation()))
            .collect())
    }

    pub async fn mob_status(&self, mob_id: &MobId) -> Result<MobState, MobError> {
        let handle = self.handle_for(mob_id).await?;
        // (b) resolver: QueryPhase carries a non-Result reply channel, so
        // its typed List deny lives here (FLAG-W-E-1 in scope_gate.rs).
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.status().await
    }

    /// Stop (pause) the mob, returning what the stop did to each member
    /// (#1500).
    pub async fn mob_stop(&self, mob_id: &MobId) -> Result<meerkat_mob::MobStopReport, MobError> {
        self.handle_for(mob_id).await?.stop().await
    }

    /// Rotate the supervisor bridge for the mob, returning the structured
    /// rotation report so RPC/MCP clients can inspect per-member outcomes
    /// instead of guessing from a bare success signal. Finding C10:
    /// `MobHandle::rotate_supervisor()` existed in the Rust API but had no
    /// operator-facing RPC surface.
    pub async fn mob_rotate_supervisor(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::SupervisorRotationReport, MobError> {
        self.handle_for(mob_id).await?.rotate_supervisor().await
    }

    pub async fn mob_resume(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.resume().await
    }

    pub async fn mob_complete(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.complete().await
    }

    pub async fn mob_reset(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.reset().await
    }

    pub async fn mob_lifecycle_action(
        &self,
        mob_id: &MobId,
        action: WireMobLifecycleAction,
    ) -> Result<MobLifecycleReports, MobMcpDestroyError> {
        match action {
            WireMobLifecycleAction::Stop => Ok(MobLifecycleReports {
                stop_report: Some(self.mob_stop(mob_id).await?),
                ..MobLifecycleReports::default()
            }),
            WireMobLifecycleAction::Resume => {
                self.mob_resume(mob_id).await?;
                Ok(MobLifecycleReports::default())
            }
            WireMobLifecycleAction::Complete => {
                self.mob_complete(mob_id).await?;
                Ok(MobLifecycleReports::default())
            }
            WireMobLifecycleAction::Reset => {
                self.mob_reset(mob_id).await?;
                Ok(MobLifecycleReports::default())
            }
            WireMobLifecycleAction::Destroy => Ok(MobLifecycleReports {
                destroy_report: Some(self.mob_destroy(mob_id).await?),
                ..MobLifecycleReports::default()
            }),
        }
    }

    /// Destroy a mob. Rejects implicit delegation mobs — use
    /// [`destroy_bridge_session_mobs`](Self::destroy_bridge_session_mobs) for
    /// bridge-session cleanup.
    ///
    /// Returns the structured [`meerkat_mob::MobDestroyReport`] only after
    /// canonical destroy completes. Partial cleanup is surfaced as
    /// [`MobMcpDestroyError::Incomplete`] with the report attached so callers
    /// can retry or inspect the still-retained mob authority.
    ///
    /// Once the native destroy completed, this state records its canonical
    /// report, the caller it was admitted for, and the storage cleanup it
    /// owes, on a task the state owns: a caller cancelled at any point loses
    /// none of them. The mob's files are removed only after its actor task
    /// ended, no handle in this process still owns its database observer,
    /// that observer drained, and the database at the path is still the
    /// mob's. Until then the destroy answers `Incomplete` with a copy of the
    /// report that names what is outstanding, an id under it is not created
    /// again, and a retry by the same caller continues the cleanup and
    /// answers the canonical report, also while another mob is managed under
    /// the id, which the retry never destroys; another caller is refused.
    /// This settles the observer and the files only: reads already entered,
    /// openers that attach later and other processes stay outside it, so it
    /// is no statement that deleting the storage was safe.
    pub async fn mob_destroy(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        self.mob_destroy_for(mob_id, MobDestroyCaller::Console, None)
            .await
    }

    /// [`Self::mob_destroy`] for `caller`, which the destroy records.
    /// `admitted_incarnation` names the mob the caller was admitted to
    /// destroy, when it was admitted against a resolved mob: the destroy is
    /// refused unless the mob it is about to destroy is that incarnation, so
    /// a mob created under the id since is never destroyed by it.
    pub(crate) async fn mob_destroy_for(
        &self,
        mob_id: &MobId,
        caller: MobDestroyCaller,
        admitted_incarnation: Option<meerkat_core::service::MobIncarnationId>,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        // A destroy this state already admitted and natively completed is
        // continued for its own caller before the actor-routed admission
        // below, which cannot admit once the actor has exited. While it owes
        // its storage cleanup it comes first, also over a mob managed under
        // the id again, which this request then never destroys.
        match self.retired_destroy_lookup(mob_id, &caller).await {
            RetiredDestroyLookup::Ordinary => {}
            RetiredDestroyLookup::OtherCaller => {
                return Err(MobMcpDestroyError::Mob(MobError::ScopeDenied(
                    ScopeDenial {
                        required: ControlScope::Retire,
                        presented: BTreeSet::new(),
                    },
                )));
            }
            RetiredDestroyLookup::Caller(retired) => {
                return self.continue_retired_destroy(mob_id, &retired).await;
            }
        }
        // The implicit-mob guard is a roster/lifecycle projection.  Admit the
        // destructive verb first so an ungranted principal cannot use the
        // guard to distinguish implicit from explicit mobs.
        self.admitted_handle_for(mob_id, ControlScope::Retire)
            .await?;
        if self.is_implicit_mob(mob_id).await {
            return Err(MobMcpDestroyError::Mob(MobError::Internal(
                "Cannot destroy implicit delegation mob directly. \
                 It is cleaned up automatically when the owning session is archived."
                    .to_string(),
            )));
        }
        self.ensure_restored().await?;
        self.mob_destroy_unchecked_loaded(mob_id, caller, admitted_incarnation)
            .await
    }

    /// The refusal of a destroy whose mob is not the incarnation it was
    /// admitted against: the caller holds no scope over the mob under the id.
    fn destroy_not_admitted() -> MobMcpDestroyError {
        MobMcpDestroyError::Mob(MobError::ScopeDenied(ScopeDenial {
            required: ControlScope::Retire,
            presented: BTreeSet::new(),
        }))
    }

    /// Destroy a mob without the implicit-mob guard.
    ///
    /// Used by session cleanup paths and canonical implicit-mob reconciliation.
    pub(crate) async fn mob_destroy_unchecked(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        self.ensure_restored().await?;
        self.mob_destroy_unchecked_loaded(mob_id, MobDestroyCaller::Console, None)
            .await
    }

    /// A destroy is refused while WorkGraph still holds an active Flow
    /// binding on the mob.
    async fn workgraph_destroy_preflight(&self, mob_id: &MobId) -> Result<(), MobError> {
        if let Some(workgraph) = self.workgraph_service.as_ref() {
            let active = workgraph
                .execution_bindings_for_recovery(None)
                .await
                .map_err(|error| MobError::Internal(error.to_string()))?
                .into_iter()
                .filter(|binding| {
                    matches!(
                        &binding.target,
                        meerkat::WorkExecutionTarget::MobFlow {
                            mob_id: bound_mob_id,
                            ..
                        } if bound_mob_id == mob_id.as_str()
                    )
                })
                .filter_map(|binding| {
                    match meerkat::WorkExecutionMachine::retry_eligible(&binding) {
                        Ok(false) => Some(binding.binding_id.to_string()),
                        Ok(true) => None,
                        Err(_) => Some(binding.binding_id.to_string()),
                    }
                })
                .collect::<Vec<_>>();
            if !active.is_empty() {
                return Err(MobError::Internal(format!(
                    "mob {mob_id} has active WorkGraph Flow bindings and cannot be destroyed: {}",
                    active.join(", ")
                )));
            }
        }
        Ok(())
    }

    async fn mob_destroy_unchecked_loaded(
        &self,
        mob_id: &MobId,
        caller: MobDestroyCaller,
        admitted_incarnation: Option<meerkat_core::service::MobIncarnationId>,
    ) -> Result<meerkat_mob::MobDestroyReport, MobMcpDestroyError> {
        // A destroy admitted against one incarnation never reaches another,
        // not even through the preflight's report of its bindings.
        if let Some(admitted) = admitted_incarnation {
            let current = self
                .mobs
                .read()
                .await
                .get(mob_id)
                .map(|managed| managed.handle.incarnation());
            if current != Some(admitted) {
                return Err(Self::destroy_not_admitted());
            }
        }
        self.workgraph_destroy_preflight(mob_id)
            .await
            .map_err(MobMcpDestroyError::Mob)?;
        let managed = {
            let mobs = self.mobs.read().await;
            mobs.get(mob_id)
                .cloned()
                .ok_or_else(|| MobMcpDestroyError::Mob(MobError::MobNotFound(mob_id.clone())))?
        };
        if admitted_incarnation.is_some_and(|admitted| managed.handle.incarnation() != admitted) {
            return Err(Self::destroy_not_admitted());
        }

        // What the destroy will owe the mob's database, taken before it
        // starts: what the path holds, and the actor task whose stores drop
        // only once it ends. A path that cannot be looked at refuses the
        // destroy before it starts.
        let actor_task = managed.handle.actor_task_watch();
        #[cfg(not(target_arch = "wasm32"))]
        let storage = match managed.storage_path.clone() {
            Some(path) => {
                let owed = OwedStorageCleanup::before_destroy(&path, actor_task.clone())
                    .await
                    .map_err(MobMcpDestroyError::Mob)?;
                Some(RetiredMobStorage {
                    path,
                    owed: Mutex::new(Some(owed)),
                })
            }
            None => None,
        };
        #[cfg(target_arch = "wasm32")]
        let storage = None;
        let destroy = AdmittedDestroy {
            mob_id: mob_id.clone(),
            caller,
            // Destroy rides the console-bound handle so chokepoint (a) sees
            // the console principal (the raw registry handle would bypass the
            // gate).
            bound: managed
                .handle
                .clone()
                .with_command_authority(CommandAuthority::principal(
                    self.console_principal.clone(),
                )),
            actor_task,
            storage,
            mobs: Arc::clone(&self.mobs),
            retired_mobs: Arc::clone(&self.retired_mobs),
            mob_set_epoch: self.mob_set_epoch.clone(),
            after_destroy: self.after_destroy.get().cloned(),
        };
        drop(managed);
        // The destroy is the state's own task, so its caller holds nothing of
        // the mob across it and cannot lose its record by being cancelled.
        // A state that stood down owns no new task: the destroy is refused
        // before it starts.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let outcome = self
                .state_tasks
                .spawn(
                    state_tasks::StateTaskKind::Owned,
                    "mob destroy",
                    destroy.run(),
                )
                .ok_or_else(|| MobMcpDestroyError::Mob(Self::stood_down_error()))?;
            outcome.await.map_err(|_| {
                MobMcpDestroyError::Mob(MobError::Internal(
                    "mob destroy ended without an answer (it panicked)".to_string(),
                ))
            })?
        }
        #[cfg(target_arch = "wasm32")]
        {
            destroy.run().await
        }
    }

    pub async fn mob_spawn(
        &self,
        mob_id: &MobId,
        profile: ProfileName,
        identity: AgentIdentity,
        runtime_mode: Option<MobRuntimeMode>,
        backend: Option<MobBackendKind>,
        placement: Option<meerkat_mob::machines::mob_machine::HostId>,
    ) -> Result<meerkat_mob::SpawnResult, MobError> {
        let mut spec = SpawnMemberSpec::host_root(profile, identity);
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        spec.placement = placement;
        self.mob_spawn_spec(mob_id, spec).await
    }

    pub async fn mob_spawn_spec(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::SpawnResult, MobError> {
        self.handle_for(mob_id).await?.spawn_spec(spec).await
    }

    pub async fn mob_spawn_many(
        &self,
        mob_id: &MobId,
        specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<Result<meerkat_mob::SpawnResult, meerkat_mob::MobSpawnManyFailure>>, MobError>
    {
        // Batch authorization is one top-level decision.  Without this gate
        // an empty batch succeeds for an ungranted caller and non-empty scope
        // denials are laundered into per-item spawn failures.
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .spawn_many(specs)
            .await
    }

    pub async fn mob_retire(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        let handle = self.handle_for(mob_id).await?;
        Self::mob_retire_on(&handle, identity).await
    }

    /// [`Self::mob_retire`] on the mob `handle` serves, which the caller
    /// already resolved and admitted against: it never resolves the id
    /// again.
    pub(crate) async fn mob_retire_on(
        handle: &MobHandle,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        // Retirement follows process-tree semantics: members this one
        // spawned (e.g. its fork_off children) are retired with it.
        handle.admit_control_scope(ControlScope::Retire).await?;
        handle.retire_with_descendants(identity).await
    }

    pub async fn mob_identity_intent(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::IdentityStoredObservation<meerkat_mob::IdentityIntentRecord>, MobError>
    {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .identity_intent(identity)
            .await
    }

    pub async fn mob_identity_convergence_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<
        meerkat_mob::IdentityStoredObservation<meerkat_mob::IdentityConvergenceStatus>,
        MobError,
    > {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .identity_convergence_status(identity)
            .await
    }

    pub async fn mob_apply_member_tool_declaration(
        &self,
        mob_id: &MobId,
        request: meerkat_mob::ApplyMemberToolDeclaration,
    ) -> Result<meerkat_mob::ApplyMemberToolDeclarationResult, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::Retire)
            .await?
            .apply_member_tool_declaration(request)
            .await
    }

    pub async fn mob_adopt_member_identity_declaration(
        &self,
        mob_id: &MobId,
        request: meerkat_mob::AdoptMemberIdentityDeclaration,
    ) -> Result<meerkat_mob::AdoptMemberIdentityDeclarationResult, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::Retire)
            .await?
            .adopt_member_identity_declaration(request)
            .await
    }

    pub async fn mob_resolve_identity_convergence_block(
        &self,
        mob_id: &MobId,
        request: meerkat_mob::ResolveIdentityConvergenceBlock,
    ) -> Result<meerkat_mob::ResolveIdentityConvergenceBlockResult, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::Retire)
            .await?
            .resolve_identity_convergence_block(request)
            .await
    }

    #[doc(hidden)]
    pub async fn archive_mob_owned_bridge_session_with_cleanup(
        &self,
        bridge_session_id: &SessionId,
        cleanup_context: &'static str,
    ) -> Result<bool, SessionError> {
        let bridge_session_key = bridge_session_id.to_string();
        let mob_owned = self.owns_live_bridge_session(bridge_session_id).await
            || self
                .owns_service_reported_bridge_session(bridge_session_id)
                .await
            || self.owns_persisted_bridge_session(bridge_session_id).await;
        if !mob_owned {
            return Ok(false);
        }

        match self
            .retire_member_by_bridge_session_id(bridge_session_id)
            .await
        {
            Ok(()) => {
                self.destroy_bridge_session_mobs(&bridge_session_key)
                    .await
                    .map_err(|error| error.into_session_error(cleanup_context))?;
                Ok(true)
            }
            Err(MobError::BridgeSessionNotInLiveAuthority { .. }) => {
                if self
                    .has_bridge_session_scoped_mobs(&bridge_session_key)
                    .await
                {
                    self.destroy_bridge_session_mobs(&bridge_session_key)
                        .await
                        .map_err(|error| error.into_session_error(cleanup_context))?;
                    Ok(true)
                } else {
                    Err(SessionError::NotFound {
                        id: bridge_session_id.clone(),
                    })
                }
            }
            Err(error) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to archive mob-owned bridge session '{bridge_session_id}': {error}"
                )),
            )),
        }
    }

    #[doc(hidden)]
    pub async fn retire_member_by_bridge_session_id(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<(), MobError> {
        self.ensure_restored().await?;
        // Derive membership from authoritative live mob roster state rather than
        // maintaining a separate reverse index that can drift during retirement,
        // respawn, or policy-driven auto-spawn flows.
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        let mut resolved = None;
        for mob_id in mob_ids {
            let roster = self.handle_for(&mob_id).await?.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                resolved = Some((mob_id, entry.agent_identity.clone()));
                break;
            }
        }
        let Some((mob_id, identity)) = resolved else {
            if self
                .owns_service_reported_bridge_session(bridge_session_id)
                .await
                || self.owns_persisted_bridge_session(bridge_session_id).await
            {
                return self
                    .session_service()
                    .archive_with_mob_lifecycle_authority(bridge_session_id)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to archive persisted mob-owned bridge session '{bridge_session_id}': {error}"
                        ))
                    });
            }
            return Err(MobError::BridgeSessionNotInLiveAuthority {
                bridge_session_id: bridge_session_id.to_string(),
            });
        };
        // Archiving a member's session is an explicit retry of the member's
        // retirement. A retirement an earlier attempt left stuck stays owned
        // by the mob and a plain retire only reports it (meerkat 0.8.51), so
        // the archive re-drives it once, then retires the rest of the tree if
        // the stuck one was a descendant.
        let retired = self.mob_retire(&mob_id, identity.clone()).await;
        let Err(MobError::MemberRetirementStuck { member_id, .. }) = retired else {
            return retired;
        };
        self.admitted_handle_for(&mob_id, ControlScope::Retire)
            .await?
            .redrive_retirement(member_id.clone())
            .await?;
        if member_id == identity {
            Ok(())
        } else {
            self.mob_retire(&mob_id, identity).await
        }
    }

    #[doc(hidden)]
    pub async fn owns_live_bridge_session(&self, bridge_session_id: &SessionId) -> bool {
        if !self
            .ensure_restored_best_effort("owns_live_bridge_session")
            .await
        {
            return false;
        }
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            if let Ok(handle) = self.handle_for(&mob_id).await
                && handle.roster().await.has_bridge_session(bridge_session_id)
            {
                return true;
            }
        }
        false
    }

    /// Resolve the exact live mob/member authority that owns a durable bridge
    /// session. No reverse index is cached, so respawn and retirement cannot
    /// leave a shadow owner behind.
    #[doc(hidden)]
    pub async fn live_member_owner(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, MobHandle, meerkat_mob::AgentIdentity)>, MobError> {
        self.ensure_restored().await?;
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            let handle = self.handle_for(&mob_id).await?;
            let roster = handle.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                let identity = entry.agent_identity.clone();
                drop(roster);
                return Ok(Some((mob_id, handle, identity)));
            }
        }
        Ok(None)
    }

    #[doc(hidden)]
    pub async fn owns_service_reported_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> bool {
        if !self
            .session_service()
            .has_live_session(bridge_session_id)
            .await
            .unwrap_or(false)
        {
            return false;
        }
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            if self
                .session_service()
                .session_belongs_to_mob(bridge_session_id, &mob_id)
                .await
            {
                return true;
            }
        }
        false
    }

    #[doc(hidden)]
    pub async fn owns_persisted_bridge_session(&self, bridge_session_id: &SessionId) -> bool {
        // Ownership routing needs only the typed identity facts on session
        // metadata — read the metadata-only seam, never the full document.
        let Some(view) = self
            .session_service()
            .load_persisted_session_metadata(bridge_session_id)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };

        let Some(mob_id) = persisted_mob_binding(&view) else {
            return false;
        };
        match self.handle_for(&mob_id).await {
            Ok(handle) => handle.roster().await.has_bridge_session(bridge_session_id),
            Err(_) => {
                self.session_service()
                    .session_belongs_to_mob(bridge_session_id, &mob_id)
                    .await
            }
        }
    }

    pub async fn mob_wire(
        &self,
        mob_id: &MobId,
        local: AgentIdentity,
        target: meerkat_mob::PeerTarget,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.wire(local, target).await
    }

    pub async fn mob_wire_members_batch(
        &self,
        mob_id: &MobId,
        edges: Vec<(AgentIdentity, AgentIdentity)>,
    ) -> Result<meerkat_mob::MobWireMembersBatchReport, MobError> {
        self.handle_for(mob_id)
            .await?
            .wire_members_batch(edges)
            .await
    }

    pub async fn mob_unwire(
        &self,
        mob_id: &MobId,
        local: AgentIdentity,
        target: meerkat_mob::PeerTarget,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.unwire(local, target).await
    }

    pub async fn mob_list_members(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<meerkat_mob::runtime::MobMemberListEntry>, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.mob_list_members_on(&handle).await
    }

    /// [`Self::mob_list_members`] on the mob `handle` serves, which the
    /// caller already resolved and admitted against.
    pub(crate) async fn mob_list_members_on(
        &self,
        handle: &MobHandle,
    ) -> Result<Vec<meerkat_mob::runtime::MobMemberListEntry>, MobError> {
        // (b) resolver: the member list is a watch-read projection that
        // never enters the actor queue (FLAG-W-E-1 in scope_gate.rs).
        self.require_console_scope(handle, ControlScope::List)?;
        handle.list_members_including_retiring().await.pipe(Ok)
    }

    /// Declaratively ensure one member after a top-level SendCommand
    /// admission.  The underlying machine first projects spawn-vs-retain;
    /// without this wrapper the retain path never reaches a scoped command.
    pub async fn mob_ensure_member(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::runtime::EnsureMemberOutcome, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .ensure_member(spec)
            .await
    }

    /// Reconcile one desired roster under its complete outer authority.
    /// Reconcile can spawn in every mode and can additionally retire when
    /// `retire_stale` is requested, so both admissions happen before the raw
    /// machine projection is evaluated.
    pub async fn mob_reconcile(
        &self,
        mob_id: &MobId,
        desired: Vec<SpawnMemberSpec>,
        options: meerkat_mob::runtime::ReconcileOptions,
    ) -> Result<meerkat_mob::runtime::ReconcileReport, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        if options.retire_stale {
            handle.admit_control_scope(ControlScope::Retire).await?;
        }
        handle.reconcile(desired, options).await
    }

    /// Filter the operational member projection under List admission before
    /// the handle reads its actor-published watch state.
    pub async fn mob_list_members_matching(
        &self,
        mob_id: &MobId,
        filter: meerkat_mob::runtime::MemberFilter,
    ) -> Result<Vec<meerkat_mob::runtime::MobMemberListEntry>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_members_matching(filter)
            .await
    }

    /// Resolve the member runtime mode and private bridge session for
    /// `mob/turn_start` only after SendCommand admission.
    pub async fn mob_turn_start_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(Option<MobRuntimeMode>, Option<SessionId>), MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let runtime_mode = handle
            .list_members()
            .await
            .into_iter()
            .find(|entry| &entry.agent_identity == identity)
            .map(|entry| entry.runtime_mode);
        let bridge_session_id = handle.resolve_bridge_session_id(identity).await;
        Ok((runtime_mode, bridge_session_id))
    }

    pub async fn mob_append_system_context(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        req: AppendSystemContextRequest,
    ) -> Result<(SessionId, AppendSystemContextResult), MobAppendSystemContextError> {
        // Session-control mechanics sit below a mob operator verb.  Enter the
        // serialized mob gate before resolving the member's private session
        // identity or touching its transcript.
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let bridge_session_id = handle
            .resolve_bridge_session_id(identity)
            .await
            .ok_or_else(|| {
                MobAppendSystemContextError::Session(SessionControlError::InvalidRequest {
                    message: format!("member has no session: {identity}"),
                })
            })?;
        let result = self
            .session_service()
            .append_system_context(&bridge_session_id, req)
            .await
            .map_err(MobAppendSystemContextError::Session)?;
        Ok((bridge_session_id, result))
    }

    pub async fn mob_resolve_bridge_session_id(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<Option<SessionId>, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .resolve_bridge_session_id(identity)
            .await)
    }

    pub async fn mob_member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<meerkat_mob::MemberDeliveryReceipt, MobError> {
        // MemberHandle acquisition reads the roster before SubmitWork reaches
        // its actor command.  Admit first so a missing member cannot become an
        // authorization oracle.
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .member(&identity)
            .await?
            .send_with_render_metadata(content, handling_mode, render_metadata)
            .await
    }

    pub async fn mob_member_send_with_external_identity(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
        delivery_identity: meerkat_mob::MobExternalDeliveryIdentity,
    ) -> Result<meerkat_mob::MemberDeliveryReceipt, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?
            .member(&identity)
            .await?
            .send_with_render_metadata_and_external_identity(
                content,
                handling_mode,
                render_metadata,
                delivery_identity,
            )
            .await
    }

    /// Deliver `content` to a member under a caller idempotency key: the one
    /// keyed member-send door for RPC, the public MCP tool and Meta.
    ///
    /// A keyed send is committed as a member continuation, so the key is kept
    /// for the member identity in the continuation key ledger, together with
    /// the committed delivery, and survives a respawn. The ledger decides a
    /// replay or a conflict before anything about the member is read, so a
    /// replay answers the first receipt even after the member was replaced.
    /// Only a first commit checks the member: it must exist and hold a
    /// session binding, because a peer-only member would never be served.
    /// The commit is bound to the mob incarnation the send was admitted
    /// against ([`meerkat::ContinuationOwnerService::submit_for_member_incarnation`]),
    /// as every operator call acts through the handle it was admitted
    /// against: a mob destroyed and created again under the id in between
    /// gets nothing committed for its member. Every answer carries the
    /// delivery status read after the commit.
    ///
    /// # Errors
    /// [`MobError::MemberSendIdempotency`] for an invalid key, a conflict, a
    /// host without a continuation service, a governed runtime, or a status
    /// read that cannot be made; [`MobError::MemberNotFound`] and
    /// [`MobError::DeliveryScopeUnavailable`] for a first commit to a member
    /// that does not exist or has no session binding, and
    /// [`MobError::MemberNotFound`] when the mob it was admitted against is no
    /// longer the one under the id.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn mob_member_send_keyed(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        handling_mode: HandlingMode,
        idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, MobError> {
        use meerkat_mob::MemberSendIdempotencyRefusal as Refusal;

        let refuse = |refusal: Refusal| MobError::MemberSendIdempotency {
            identity: identity.clone(),
            refusal,
        };
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        meerkat_runtime::validate_canonical_key_text(
            "idempotency_key",
            idempotency_key,
            meerkat_runtime::CALLER_IDEMPOTENCY_KEY_MAX_BYTES,
        )
        .map_err(|error| {
            refuse(Refusal::InvalidKey {
                detail: error.to_string(),
            })
        })?;
        let service = self
            .continuation_service()
            .ok_or_else(|| refuse(Refusal::NoContinuationService))?;
        if service.governs_work_authority() {
            return Err(refuse(Refusal::GovernedRuntime));
        }
        let owner = meerkat::ContinuationOwner::Member {
            mob_id: mob_id.to_string(),
            identity: identity.to_string(),
        };
        let key = meerkat::ContinuationKey::new(format!(
            "{MEMBER_SEND_CONTINUATION_KEY_PREFIX}{idempotency_key}"
        ))
        .map_err(|error| {
            refuse(Refusal::InvalidKey {
                detail: error.to_string(),
            })
        })?;
        let status_error = |error: meerkat::ContinuationStatusError| match error {
            meerkat::ContinuationStatusError::Unavailable(detail) => {
                refuse(Refusal::Unavailable { detail })
            }
            meerkat::ContinuationStatusError::Inconsistent(detail) => {
                refuse(Refusal::Inconsistent { detail })
            }
        };
        if matches!(
            service
                .continuation_status(&owner, &key)
                .await
                .map_err(status_error)?,
            meerkat::ContinuationStatus::NotCommitted
        ) {
            handle.capture_member_delivery_scope(&identity).await?;
        }
        let content_bytes = serde_json::to_vec(&content).map_err(|error| {
            refuse(Refusal::Inconsistent {
                detail: format!("member send content does not serialize: {error}"),
            })
        })?;
        let delivery = meerkat::ContinuationDelivery {
            key: key.clone(),
            result: meerkat::ContinuationResultRef {
                producer: meerkat::ContinuationProducer::Host {
                    namespace: MEMBER_SEND_CONTINUATION_NAMESPACE.to_string(),
                },
                producer_id: idempotency_key.to_string(),
                result_digest: meerkat_runtime::delivery_digest_hex(&[
                    MEMBER_SEND_CONTINUATION_NAMESPACE.as_bytes(),
                    &content_bytes,
                ]),
                summary: None,
            },
            body: meerkat::ContinuationBody::Content { content },
            handling: match handling_mode {
                HandlingMode::Queue => meerkat::ContinuationHandling::Queue,
                HandlingMode::Steer => meerkat::ContinuationHandling::Steer,
            },
        };
        let committed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        self.pause_agent_call().await;
        let receipt = service
            .submit_for_member_incarnation(&owner, handle.incarnation(), delivery, committed_at_ms)
            .await
            .map_err(|error| match error {
                meerkat::ContinuationSubmitError::Conflict { existing } => {
                    refuse(Refusal::Conflict {
                        delivery_id: existing.delivery_id.clone(),
                        committed_at_ms: existing.committed_at_ms,
                    })
                }
                meerkat::ContinuationSubmitError::OwnerRetired => {
                    MobError::MemberNotFound(identity.clone())
                }
                meerkat::ContinuationSubmitError::NoAdmissibleWorkBinding => {
                    refuse(Refusal::GovernedRuntime)
                }
                meerkat::ContinuationSubmitError::Invalid(detail) => {
                    refuse(Refusal::InvalidKey { detail })
                }
                meerkat::ContinuationSubmitError::Unavailable(detail) => {
                    refuse(Refusal::Unavailable { detail })
                }
            })?;
        let status = match service
            .continuation_status(&owner, &key)
            .await
            .map_err(status_error)?
        {
            meerkat::ContinuationStatus::NotCommitted => {
                return Err(refuse(Refusal::Inconsistent {
                    detail: "the key reads as not committed after its commit".to_string(),
                }));
            }
            meerkat::ContinuationStatus::Pending { admitted: None, .. } => {
                meerkat_contracts::WireContinuationStatus::Pending
            }
            meerkat::ContinuationStatus::Pending {
                admitted: Some((session, input)),
                ..
            }
            | meerkat::ContinuationStatus::Applied { session, input, .. } => {
                meerkat_contracts::WireContinuationStatus::Admitted {
                    session_id: session.to_string(),
                    input_id: input.to_string(),
                }
            }
            meerkat::ContinuationStatus::Stranded { .. } => {
                meerkat_contracts::WireContinuationStatus::Stranded
            }
            meerkat::ContinuationStatus::Refused { reason, .. } => {
                meerkat_contracts::WireContinuationStatus::Refused {
                    reason: serde_json::to_value(reason)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_else(|| format!("{reason:?}")),
                }
            }
        };
        Ok(meerkat_contracts::WireContinuationReceipt {
            delivery_id: receipt.delivery_id,
            committed_at_ms: receipt.committed_at_ms,
            status,
        })
    }

    /// [`Self::mob_member_send_keyed`] on a host that binds no continuation
    /// service (the browser runtime): the key cannot be kept, so it is
    /// refused after authorization, never dropped.
    #[cfg(target_arch = "wasm32")]
    pub async fn mob_member_send_keyed(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        _content: ContentInput,
        _handling_mode: HandlingMode,
        _idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        Err(MobError::MemberSendIdempotency {
            identity,
            refusal: meerkat_mob::MemberSendIdempotencyRefusal::NoContinuationService,
        })
    }

    pub async fn mob_ingress_interaction(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
        content: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<MobIngressInteractionOutcome, MobError> {
        let identity = spec.identity.clone();
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SendCommand)
            .await?;
        let events_after_cursor = handle.events().latest_cursor().await?;
        let ensure_outcome = handle.ensure_member(spec).await?;
        let delivery = handle
            .member(&identity)
            .await?
            .send_with_render_metadata(content, handling_mode, render_metadata)
            .await?;
        let latest_event_cursor = handle.events().latest_cursor().await?;
        Ok(MobIngressInteractionOutcome {
            ensure_outcome,
            delivery,
            events_after_cursor,
            latest_event_cursor,
        })
    }

    pub async fn mob_events(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?
            .events()
            .poll(after_cursor, limit)
            .await
    }

    pub async fn mob_events_strict(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        // Strict polling reads the latest cursor before its ordinary PollEvents
        // command.  Gate before that read so ScopeDenied wins over StaleCursor
        // and the destroyed-mob event-store fallback remains protected.
        self.admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?
            .events()
            .poll_strict(after_cursor, limit)
            .await
    }

    pub async fn mob_latest_event_cursor(&self, mob_id: &MobId) -> Result<u64, MobError> {
        let handle = self.handle_for(mob_id).await?;
        // (b) resolver: the cursor read is an event-store read outside the
        // actor queue — same class as the subscription verbs.
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.events().latest_cursor().await
    }

    /// Submit a unit of work to a mob member through the work-lane.
    ///
    /// Thin wrapper over [`meerkat_mob::MobHandle::submit_work`] for the
    /// mob-surface crate. Finding C4 — the work-lane Rust API
    /// (`submit_work`/`cancel_work`/`cancel_all_work`) was Rust-only;
    /// this exposes it to RPC/HTTP consumers such as meta.
    pub async fn mob_submit_work(
        &self,
        mob_id: &MobId,
        runtime_id: meerkat_mob::AgentRuntimeId,
        fence_token: meerkat_mob::FenceToken,
        work_ref: meerkat_mob::WorkRef,
        spec: meerkat_mob::WorkSpec,
    ) -> Result<meerkat_mob::WorkDeliveryReceipt, MobError> {
        self.handle_for(mob_id)
            .await?
            .submit_work(runtime_id, fence_token, work_ref, spec)
            .await
    }

    /// Explicitly conclude the kickoff objective owned by a member.
    pub async fn mob_conclude_objective(
        &self,
        mob_id: &MobId,
        identity: &meerkat_mob::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
        outcome: impl Into<String>,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .conclude_objective(identity, objective_id, outcome)
            .await
    }

    pub async fn mob_bind_objective_owner(
        &self,
        mob_id: &MobId,
        owner_identity: meerkat_mob::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .bind_objective_owner(owner_identity, objective_id)
            .await
    }

    fn objective_owner_identity_for_session(
        bridge_session_id: &SessionId,
    ) -> meerkat_mob::AgentIdentity {
        meerkat_mob::AgentIdentity::objective_lead_for_session(bridge_session_id)
    }

    pub async fn objective_principal_for_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, meerkat_mob::AgentIdentity)>, MobError> {
        self.ensure_restored().await?;
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        // A real roster binding is authoritative when the same session also
        // owns an implicit child mob: conclusions belong in the lead's mob.
        for mob_id in &mob_ids {
            let roster = self.handle_for(mob_id).await?.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
                return Ok(Some((mob_id.clone(), entry.agent_identity.clone())));
            }
        }
        for mob_id in mob_ids {
            let handle = self.handle_for(&mob_id).await?;
            if handle.owns_bridge_session(bridge_session_id).await? {
                return Ok(Some((
                    mob_id,
                    Self::objective_owner_identity_for_session(bridge_session_id),
                )));
            }
        }
        Ok(None)
    }

    pub async fn objective_principal_for_mob_owner_session(
        &self,
        mob_id: &MobId,
        bridge_session_id: &SessionId,
    ) -> Result<meerkat_mob::AgentIdentity, MobError> {
        let handle = self.handle_for(mob_id).await?;
        Self::objective_principal_on(&handle, bridge_session_id).await
    }

    /// [`Self::objective_principal_for_mob_owner_session`] in the mob
    /// `handle` serves, which the caller already resolved.
    pub(crate) async fn objective_principal_on(
        handle: &MobHandle,
        bridge_session_id: &SessionId,
    ) -> Result<meerkat_mob::AgentIdentity, MobError> {
        let roster = handle.roster().await;
        if let Some(entry) = roster.find_by_bridge_session_id(bridge_session_id) {
            return Ok(entry.agent_identity.clone());
        }
        drop(roster);
        if handle.owns_bridge_session(bridge_session_id).await? {
            return Ok(Self::objective_owner_identity_for_session(
                bridge_session_id,
            ));
        }
        Err(MobError::Internal(format!(
            "bridge session {bridge_session_id} is not an objective lead for mob {}",
            handle.mob_id()
        )))
    }

    #[doc(hidden)]
    pub async fn member_for_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, meerkat_mob::AgentIdentity)>, MobError> {
        Ok(self
            .member_handle_for_bridge_session(bridge_session_id)
            .await?
            .map(|(mob_id, _, identity)| (mob_id, identity)))
    }

    /// The member seated on `bridge_session_id` in a mob this state manages,
    /// with that mob's handle. A mob removed while the read runs (a
    /// concurrent destroy) is skipped, not an error.
    pub(crate) async fn member_handle_for_bridge_session(
        &self,
        bridge_session_id: &SessionId,
    ) -> Result<Option<(MobId, MobHandle, meerkat_mob::AgentIdentity)>, MobError> {
        self.ensure_restored().await?;
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            let handle = match self.handle_for(&mob_id).await {
                Ok(handle) => handle,
                Err(MobError::MobNotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            if let Some(entry) = handle
                .roster()
                .await
                .find_by_bridge_session_id(bridge_session_id)
            {
                let identity = entry.agent_identity.clone();
                return Ok(Some((mob_id, handle, identity)));
            }
        }
        Ok(None)
    }

    /// Resolve the current runtime binding for an opaque member reference
    /// under the scope of the operation that will consume it.
    ///
    /// This deliberately does not call `mob_list_members`: resolving a write
    /// target must not secretly require `List`, and a denied write must fail
    /// before revealing whether the member exists.
    async fn member_binding_after_admission(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        required: ControlScope,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        let handle = self.admitted_handle_for(mob_id, required).await?;
        let entry = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| &entry.agent_identity == identity)
            .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
        entry.binding_atoms().ok_or_else(|| {
            MobError::Internal(format!(
                "member {identity} has no MobMachine runtime binding"
            ))
        })
    }

    /// Resolve a member binding for a SendCommand-class operation without
    /// imposing the unrelated List scope.
    pub async fn mob_member_binding_for_send_command(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        self.member_binding_after_admission(mob_id, identity, ControlScope::SendCommand)
            .await
    }

    /// Resolve a member binding for a Cancel-class operation without
    /// imposing the unrelated List scope.
    pub async fn mob_member_binding_for_cancel(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError> {
        self.member_binding_after_admission(mob_id, identity, ControlScope::Cancel)
            .await
    }

    /// Cancel a previously submitted unit of work. Finding C4.
    pub async fn mob_cancel_work(
        &self,
        mob_id: &MobId,
        work_ref: meerkat_mob::WorkRef,
    ) -> Result<(), MobError> {
        // Per-item cancellation is intentionally unsupported, but the
        // authorization decision still precedes that honest capability error.
        self.admitted_handle_for(mob_id, ControlScope::Cancel)
            .await?
            .cancel_work(work_ref)
            .await
    }

    /// Cancel all in-flight work for a specific mob member. Finding C4.
    pub async fn mob_cancel_all_work(
        &self,
        mob_id: &MobId,
        runtime_id: meerkat_mob::AgentRuntimeId,
        fence_token: meerkat_mob::FenceToken,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .cancel_all_work(runtime_id, fence_token)
            .await
    }

    pub async fn mob_list_flows(&self, mob_id: &MobId) -> Result<Vec<String>, MobError> {
        let flows = self
            .admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_flows();
        Ok(flows
            .into_iter()
            .map(|flow_id| flow_id.to_string())
            .collect())
    }

    pub async fn mob_run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.mob_run_flow_with_stream(mob_id, flow_id, params, None)
            .await
    }

    pub async fn mob_run_flow_with_external_identity(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        identity: &meerkat_mob::MobExternalDeliveryIdentity,
    ) -> Result<RunId, MobError> {
        self.handle_for(mob_id)
            .await?
            .run_flow_with_external_identity(flow_id, params, identity)
            .await
    }

    pub async fn mob_begin_external_delivery(
        &self,
        intent: &meerkat_mob::MobExternalDeliveryIntent,
    ) -> Result<meerkat_mob::MobExternalDeliveryBeginOutcome, MobError> {
        self.handle_for(&intent.mob_id)
            .await?
            .begin_external_delivery(intent)
            .await
    }

    pub async fn mob_complete_external_delivery(
        &self,
        intent: &meerkat_mob::MobExternalDeliveryIntent,
        terminal: &meerkat_mob::MobExternalDeliveryTerminal,
    ) -> Result<meerkat_mob::MobExternalDeliveryCompleteOutcome, MobError> {
        self.handle_for(&intent.mob_id)
            .await?
            .complete_external_delivery(intent, terminal)
            .await
    }

    pub async fn mob_schedule_external_delivery_repair(
        &self,
        intent: &meerkat_mob::MobExternalDeliveryIntent,
    ) -> Result<meerkat_mob::MobExternalDeliveryRepairOutcome, MobError> {
        self.handle_for(&intent.mob_id)
            .await?
            .schedule_external_delivery_repair(intent)
            .await
    }

    pub async fn mob_run_flow_with_stream(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        // `run_flow_with_stream`'s preview command is the SendCommand-scoped
        // outer admission and runs before any flow-target provisioning read.
        self.handle_for(mob_id)
            .await?
            .run_flow_with_stream(flow_id, params, scoped_event_tx)
            .await
    }

    pub async fn mob_flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<meerkat_mob::MobRun>, MobError> {
        self.handle_for(mob_id).await?.flow_status(run_id).await
    }

    /// Project token accounting and transcript pointers for a mob.
    ///
    /// The mob run aggregate records no token usage (`MobRun` has no usage
    /// field, and the flow turn bridge drops the `RunCompleted` usage it
    /// observes), so the only canonical usage authority reachable here is each
    /// member's bridge session. That makes every number in this projection
    /// session-cumulative, which
    /// [`meerkat_contracts::WireMobRunUsageAttribution`] states explicitly
    /// rather than implying per-run attribution. Members whose session cannot
    /// be read locally (remote placement, archived session) are reported with
    /// `usage_unavailable` instead of a zero that would read as "cost nothing".
    ///
    /// Full transcripts, tool calls, and per-turn usage are NOT projected
    /// here: `member_session_ids` is the pointer set for
    /// `rkat session export-atif`, which is the single transcript authority.
    ///
    /// Per-member absence degrades per member, never for the whole mob: a
    /// member whose status or session cannot be resolved is projected with a
    /// `usage_unavailable` reason. Only a failure to read the roster is fatal,
    /// because without it there is no member set to project.
    pub async fn mob_run_accounting(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_contracts::WireMobRunAccounting, MobError> {
        let members = self.mob_list_members(mob_id).await?;
        let mut inputs = Vec::with_capacity(members.len());
        for member in members {
            let session = match self.mob_member_status(mob_id, &member.agent_identity).await {
                Err(error) => run_accounting::MobMemberSessionUsage::StatusUnresolved {
                    reason: format!("member status is not resolvable here: {error}"),
                },
                Ok(snapshot) => match snapshot.current_session_id.as_ref() {
                    None => run_accounting::MobMemberSessionUsage::NoSession,
                    Some(session_id) => match self.session_service.read(session_id).await {
                        Ok(view) => run_accounting::MobMemberSessionUsage::Read {
                            session_id: session_id.to_string(),
                            model: view.state.model.clone(),
                            provider: view.state.provider,
                            message_count: view.state.message_count as u64,
                            usage: view.billing.usage,
                        },
                        Err(error) => run_accounting::MobMemberSessionUsage::Unreadable {
                            session_id: session_id.to_string(),
                            reason: format!("member session is not readable here: {error}"),
                        },
                    },
                },
            };
            inputs.push(run_accounting::MobMemberUsageInput {
                agent_identity: member.agent_identity.as_str().to_string(),
                role: member.role.as_str().to_string(),
                session,
            });
        }
        Ok(run_accounting::mob_run_accounting_projection(inputs))
    }

    /// Resolve the redacted WorkGraph linkage for one exact Flow run.
    pub async fn workgraph_execution_binding_for_flow_run(
        &self,
        mob_id: &MobId,
        run_id: &RunId,
    ) -> Result<Option<meerkat::WorkExecutionBinding>, MobError> {
        let Some(workgraph) = self.workgraph_service.as_ref() else {
            return Ok(None);
        };
        let binding = workgraph
            .execution_binding_for_target_run(&run_id.to_string())
            .await
            .map_err(|error| MobError::Internal(error.to_string()))?;
        if let Some(binding) = binding.as_ref() {
            let meerkat::WorkExecutionTarget::MobFlow {
                mob_id: bound_mob_id,
                ..
            } = &binding.target;
            if bound_mob_id != mob_id.as_str() {
                return Err(MobError::Internal(format!(
                    "WorkGraph execution run '{run_id}' belongs to mob '{bound_mob_id}' rather than requested mob '{mob_id}'"
                )));
            }
        }
        Ok(binding)
    }

    pub async fn mob_list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<meerkat_mob::MobRun>, MobError> {
        self.admitted_handle_for(mob_id, ControlScope::List)
            .await?
            .list_runs(flow_id)
            .await
    }

    pub async fn mob_cancel_flow(&self, mob_id: &MobId, run_id: RunId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.cancel_flow(run_id).await
    }

    pub async fn mob_respawn(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        initial_message: Option<meerkat_core::types::ContentInput>,
    ) -> Result<meerkat_mob::MemberRespawnReceipt, meerkat_mob::MobRespawnError> {
        let handle = self.handle_for(mob_id).await?;
        handle.respawn(identity, initial_message).await
    }

    pub async fn mob_force_cancel(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .force_cancel_member(identity)
            .await
    }

    pub async fn mob_member_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MobMemberSnapshot, MobError> {
        let handle = self.handle_for(mob_id).await?;
        Self::mob_member_status_on(&handle, identity).await
    }

    /// [`Self::mob_member_status`] on the mob `handle` serves, which the
    /// caller already resolved and admitted against.
    pub(crate) async fn mob_member_status_on(
        handle: &MobHandle,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MobMemberSnapshot, MobError> {
        // The retiring-member fast path reads a machine watch and can return
        // before ProjectMemberStatus reaches the actor gate.  Admit first so
        // every lifecycle phase has identical denial precedence.
        handle.admit_control_scope(ControlScope::List).await?;
        handle.member_status(identity).await
    }

    pub async fn realtime_validate_session_target(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.session_service.read(session_id).await.map(|_| ())
    }

    pub async fn mob_wait_kickoff(
        &self,
        mob_id: &MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<KickoffMemberSnapshot>, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?;
        // Admission is the authorization event for the composite wait.  Its
        // internal snapshot collection must not impose an unrelated `List`
        // grant after the wait has already been admitted.
        let admitted =
            handle.with_command_authority(CommandAuthority::principal(MobControlPrincipal::Owner));
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => {
                admitted
                    .wait_for_members_kickoff_complete(&ids, timeout)
                    .await?
            }
            None => admitted.wait_for_kickoff_complete(timeout).await?,
        };
        Ok(snapshots
            .into_iter()
            .map(|(identity, snapshot)| KickoffMemberSnapshot {
                agent_identity: identity,
                snapshot,
            })
            .collect())
    }

    pub async fn mob_wait_ready(
        &self,
        mob_id: &MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Result<Vec<KickoffMemberSnapshot>, MobError> {
        let handle = self
            .admitted_handle_for(mob_id, ControlScope::SubscribeEvents)
            .await?;
        let admitted =
            handle.with_command_authority(CommandAuthority::principal(MobControlPrincipal::Owner));
        let timeout = timeout_ms.map(Duration::from_millis);
        let snapshots = match member_ids {
            Some(ids) => admitted.wait_for_members_ready(&ids, timeout).await?,
            None => admitted.wait_for_ready(timeout).await?,
        };
        Ok(snapshots
            .into_iter()
            .map(|(identity, snapshot)| KickoffMemberSnapshot {
                agent_identity: identity,
                snapshot,
            })
            .collect())
    }

    pub async fn mob_spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: meerkat_mob::HelperOptions,
        result_label: impl Into<String>,
        max_text_bytes: usize,
    ) -> Result<meerkat_mob::BoundedHelperRunOutcome, meerkat_mob::BoundedMemberRunError> {
        self.handle_for(mob_id)
            .await?
            .spawn_helper(identity, prompt, options, result_label, max_text_bytes)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn mob_fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: meerkat_mob::ForkContext,
        options: meerkat_mob::HelperOptions,
        result_label: impl Into<String>,
        max_text_bytes: usize,
    ) -> Result<meerkat_mob::BoundedHelperRunOutcome, meerkat_mob::BoundedMemberRunError> {
        self.handle_for(mob_id)
            .await?
            .fork_helper(
                source_identity,
                identity,
                prompt,
                fork_context,
                options,
                result_label,
                max_text_bytes,
            )
            .await
    }

    /// Subscribe to mob-wide events (all members, continuously updated).
    ///
    /// (b) gate at subscription ADMISSION only: a later revoke/expiry does
    /// not tear down an already-open stream in v1 (ADJ-P5-17).
    pub async fn subscribe_mob_events(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobEventRouterHandle, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.subscribe_mob_events().await
    }

    /// Subscribe to agent-level events for a specific member.
    ///
    /// (b) gate at subscription admission only (ADJ-P5-17).
    pub async fn subscribe_agent_events(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<meerkat_core::comms::EventStream, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.subscribe_agent_events(identity).await
    }

    /// Subscribe to agent-level events for a specific member, starting at a
    /// typed cursor; see [`meerkat_mob::MobHandle::subscribe_agent_events_from`].
    ///
    /// (b) gate at subscription admission only (ADJ-P5-17).
    pub async fn subscribe_agent_events_from(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<meerkat_mob::AgentEventSubscription, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::SubscribeEvents)?;
        handle.subscribe_agent_events_from(identity, cursor).await
    }

    /// Find the implicit delegation mob for the given bridge session, if one exists.
    ///
    /// Scans the in-memory mob registry for a mob whose generated MobMachine
    /// owner-bridge authority marks it implicit and indexed to the given bridge
    /// session ID.
    /// Does NOT match explicit mobs that merely share the same owner.
    #[doc(hidden)]
    pub async fn find_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
    ) -> Option<MobId> {
        let bridge_session_id = SessionId::parse(bridge_session_id).ok()?;
        if !self
            .ensure_restored_best_effort("find_implicit_mob_for_bridge_session")
            .await
        {
            return None;
        }
        self.implicit_mob_in_registry(&bridge_session_id).await
    }

    /// The implicit delegation mob of `bridge_session_id` among the mobs this
    /// state already manages, without restoring anything: `None` until the
    /// managed set is complete (the persistent restore finished, or the host
    /// declared its own restore done).
    ///
    /// Agent mob tool construction seeds its implicit-mob hint with this. A
    /// persisted member is rebuilt, mob tools included, inside the persistent
    /// restore, which holds the restore lock; a construction-time lookup that
    /// restored would wait on the restore it is part of. The hint is only a
    /// hint: every use goes through [`Self::ensure_implicit_mob_for_model`],
    /// which validates it and otherwise restores and looks the mob up under
    /// the per-session lock before it would create one.
    #[doc(hidden)]
    pub async fn restored_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
    ) -> Option<MobId> {
        let bridge_session_id = SessionId::parse(bridge_session_id).ok()?;
        if !self.mob_set_complete() {
            return None;
        }
        self.implicit_mob_in_registry(&bridge_session_id).await
    }

    async fn implicit_mob_in_registry(&self, bridge_session_id: &SessionId) -> Option<MobId> {
        let mobs = self.mobs.read().await;
        mobs.iter()
            .find(|(_, m)| {
                m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| {
                        authority.implicit_delegation_mob
                            && authority.bridge_session_id == *bridge_session_id
                    })
            })
            .map(|(id, _)| id.clone())
    }

    /// Check whether the given mob is an implicit delegation mob.
    ///
    /// Checks generated MobMachine owner-bridge authority.
    #[doc(hidden)]
    pub async fn is_implicit_mob(&self, mob_id: &MobId) -> bool {
        if !self.ensure_restored_best_effort("is_implicit_mob").await {
            return false;
        }
        let mobs = self.mobs.read().await;
        mobs.get(mob_id)
            .and_then(|m| m.handle.owner_bridge_session_lifecycle_authority())
            .is_some_and(|authority| authority.implicit_delegation_mob)
    }

    /// Find all mobs indexed to the given owner bridge session
    /// (both implicit and explicit).
    #[doc(hidden)]
    pub async fn find_mobs_for_bridge_session(&self, bridge_session_id: &str) -> Vec<MobId> {
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return Vec::new(),
        };
        if !self
            .ensure_restored_best_effort("find_mobs_for_bridge_session")
            .await
        {
            return Vec::new();
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .filter_map(|(id, m)| {
                if m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| authority.bridge_session_id == bridge_session_id)
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn find_bridge_session_scoped_mobs(&self, bridge_session_id: &str) -> Vec<MobId> {
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return Vec::new(),
        };
        if !self
            .ensure_restored_best_effort("find_bridge_session_scoped_mobs")
            .await
        {
            return Vec::new();
        }
        let mobs = self.mobs.read().await;
        mobs.iter()
            .filter_map(|(id, m)| {
                if m.handle
                    .owner_bridge_session_lifecycle_authority()
                    .is_some_and(|authority| {
                        authority.destroy_on_owner_archive
                            && authority.bridge_session_id == bridge_session_id
                    })
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[doc(hidden)]
    pub async fn has_bridge_session_scoped_mobs(&self, bridge_session_id: &str) -> bool {
        !self
            .find_bridge_session_scoped_mobs(bridge_session_id)
            .await
            .is_empty()
    }

    async fn implicit_mob_matches_session_model(
        &self,
        mob_id: &MobId,
        bridge_session_id: &str,
        model: &str,
    ) -> bool {
        let delegate_profile = ProfileName::from("delegate");
        let bridge_session_id = match SessionId::parse(bridge_session_id) {
            Ok(session_id) => session_id,
            Err(_) => return false,
        };
        let mobs = self.mobs.read().await;
        mobs.get(mob_id).is_some_and(|managed| {
            let definition = managed.handle.definition();
            managed
                .handle
                .owner_bridge_session_lifecycle_authority()
                .is_some_and(|authority| {
                    authority.implicit_delegation_mob
                        && authority.bridge_session_id == bridge_session_id
                })
                && definition
                    .resolve_inline_profile(&delegate_profile)
                    .is_some_and(|profile| profile.model == model)
        })
    }

    /// Ensure the canonical implicit delegation mob exists for the given
    /// owner bridge session/model pair.
    ///
    /// This is the sole owner of implicit-mob model reconciliation. Tool
    /// surfaces may provide a cached mob-id hint for the fast path, but they
    /// must not destroy or recreate mobs themselves.
    #[doc(hidden)]
    pub async fn ensure_implicit_mob_for_model(
        &self,
        bridge_session_id: &str,
        model: &str,
        cached_mob_id: Option<&MobId>,
    ) -> Result<(MobId, bool), MobError> {
        if let Some(cached_mob_id) = cached_mob_id
            && self
                .implicit_mob_matches_session_model(cached_mob_id, bridge_session_id, model)
                .await
        {
            return Ok((cached_mob_id.clone(), false));
        }

        // Get or create a per-owner-bridge-session lock
        let session_lock = {
            let mut locks = self.implicit_mob_locks.lock().await;
            locks
                .entry(bridge_session_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = session_lock.lock().await;

        if let Some(mob_id) = self
            .find_implicit_mob_for_bridge_session(bridge_session_id)
            .await
        {
            if self
                .implicit_mob_matches_session_model(&mob_id, bridge_session_id, model)
                .await
            {
                return Ok((mob_id, false));
            }
            self.mob_destroy_unchecked(&mob_id)
                .await
                .map_err(MobMcpDestroyError::into_mob_error)?;
        }

        let owner_bridge_session_id = SessionId::parse(bridge_session_id).map_err(|error| {
            MobError::Internal(format!(
                "invalid implicit mob owner bridge session id '{bridge_session_id}': {error}"
            ))
        })?;
        let mob_id = self
            .mob_create_definition_with_owner_bridge_session(
                MobDefinition::implicit(bridge_session_id, model),
                owner_bridge_session_id,
                true,
                true,
            )
            .await?;
        Ok((mob_id, true))
    }

    /// Get or create the implicit delegation mob for the given owner bridge
    /// session.
    ///
    /// Uses a per-bridge-session mutex to ensure single-flight creation: the first
    /// caller creates the mob, concurrent callers block and receive the same
    /// mob ID. The mob is indexed to the owning bridge session and marked
    /// bridge-session-scoped.
    #[doc(hidden)]
    pub async fn get_or_create_implicit_mob_for_bridge_session(
        &self,
        bridge_session_id: &str,
        model: &str,
    ) -> Result<MobId, MobError> {
        self.ensure_implicit_mob_for_model(bridge_session_id, model, None)
            .await
            .map(|(mob_id, _created)| mob_id)
    }

    /// Destroy all bridge-session-scoped mobs for the given owner bridge session.
    ///
    /// Called during session archive to clean up mobs whose cleanup truth is
    /// `DestroyOnOwnerArchive`. Owner indexing alone is not sufficient.
    #[doc(hidden)]
    pub async fn destroy_bridge_session_mobs(
        &self,
        bridge_session_id: &str,
    ) -> Result<(), MobMcpDestroyError> {
        self.ensure_restored().await?;
        let mob_ids = self
            .find_bridge_session_scoped_mobs(bridge_session_id)
            .await;
        if mob_ids.is_empty() {
            return Ok(());
        }
        let mut first_error = None;
        for mob_id in &mob_ids {
            if let Err(error) = self.mob_destroy_unchecked(mob_id).await {
                tracing::warn!(
                    mob_id = %mob_id,
                    bridge_session_id = %bridge_session_id,
                    error = %error,
                    "failed to destroy bridge-session-scoped mob during cleanup"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        // Prune the per-bridge-session lock to avoid unbounded growth in long-lived processes.
        let mut locks = self.implicit_mob_locks.lock().await;
        locks.remove(bridge_session_id);
        Ok(())
    }

    /// Scavenge bridge-session-scoped mobs whose owning session is archived.
    ///
    /// Called during hydration at startup. For each mob marked
    /// `DestroyOnOwnerArchive`, observes the owning bridge session's durable
    /// source and destroys the mob only when the owner is confirmed archived.
    /// An owner with no durable authority (`Absent`) or one that cannot be
    /// observed is not an archive: the mob is kept as it is. Returns the list
    /// of scavenged mob IDs.
    #[doc(hidden)]
    pub async fn scavenge_orphaned_bridge_session_scoped_mobs(&self) -> Vec<MobId> {
        if !self
            .ensure_restored_best_effort("scavenge_orphaned_bridge_session_scoped_mobs")
            .await
        {
            return Vec::new();
        }
        self.scavenge_orphaned_bridge_session_scoped_mobs_inner()
            .await
    }

    /// Compatibility wrapper for callers that still use session-centric naming.
    pub async fn scavenge_orphaned_session_scoped_mobs(&self) -> Vec<MobId> {
        self.scavenge_orphaned_bridge_session_scoped_mobs().await
    }

    async fn scavenge_orphaned_bridge_session_scoped_mobs_inner(&self) -> Vec<MobId> {
        // The reobservation and each destroy below are built in their own
        // frames: at opt-level 0 each awaited future reserved twice its size
        // in this poll frame.
        #[cfg(not(target_arch = "wasm32"))]
        let mut scavenged =
            meerkat_runtime::stack_relief::box_in_own_frame(|| self.reobserve_held_child_mobs())
                .await;
        #[cfg(target_arch = "wasm32")]
        let mut scavenged = Vec::new();
        // Collect bridge-session-scoped cleanup candidates under a read lock.
        let candidates: Vec<(MobId, SessionId)> = {
            let mobs = self.mobs.read().await;
            mobs.iter()
                .filter_map(|(id, m)| {
                    m.handle
                        .owner_bridge_session_lifecycle_authority()
                        .filter(|authority| authority.destroy_on_owner_archive)
                        .map(|authority| (id.clone(), authority.bridge_session_id))
                })
                .collect()
        };

        for (mob_id, bridge_session_id) in candidates {
            // Only a confirmed archive authorizes owner-driven destruction. A
            // session that reads as not found may merely have no durable
            // authority yet, and an observation fault proves nothing; both
            // keep the child.
            let owner_archived = match self
                .session_service
                .observe_live_durable_source(&bridge_session_id)
                .await
            {
                Ok(meerkat_mob::LiveDurableSourceObservation::Archived) => true,
                Ok(_) => false,
                Err(error) => {
                    tracing::warn!(
                        mob_id = %mob_id,
                        bridge_session_id = %bridge_session_id,
                        error = %error,
                        "could not observe the owner of a bridge-session-scoped mob; keeping it"
                    );
                    false
                }
            };
            if owner_archived {
                if let Err(error) = meerkat_runtime::stack_relief::box_in_own_frame(|| {
                    self.mob_destroy_unchecked_loaded(&mob_id, MobDestroyCaller::Console, None)
                })
                .await
                {
                    tracing::warn!(
                        mob_id = %mob_id,
                        bridge_session_id = %bridge_session_id,
                        error = %error,
                        "failed to scavenge orphaned bridge-session-scoped mob"
                    );
                } else {
                    tracing::info!(
                        mob_id = %mob_id,
                        bridge_session_id = %bridge_session_id,
                        "scavenged orphaned bridge-session-scoped mob"
                    );
                    scavenged.push(mob_id);
                    // Prune per-bridge-session lock to avoid unbounded growth.
                    let mut locks = self.implicit_mob_locks.lock().await;
                    locks.remove(&bridge_session_id.to_string());
                }
            }
        }
        scavenged
    }

    /// Look up the [`MobHandle`] for a given mob ID.
    ///
    /// Returns `MobError::MobNotFound` if the mob is not found.
    /// Every handle this console hands out is rebound to the console
    /// principal (chokepoint (b) principal minting, DEC-P5E-8): actor-routed
    /// verbs are then gated once, at chokepoint (a), against this principal.
    pub async fn handle_for(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        self.ensure_restored().await?;
        let handle = self.mobs.read().await.get(mob_id).map(|m| {
            m.handle
                .clone()
                .with_command_authority(CommandAuthority::principal(self.console_principal.clone()))
        });
        match handle {
            Some(handle) => Ok(handle),
            None => Err(self
                .unmanaged_mob_error(mob_id)
                .await
                .unwrap_or_else(|| MobError::MobNotFound(mob_id.clone()))),
        }
    }

    /// Resolve a console-bound handle and enter the actor-linearized scope
    /// gate before a composite surface performs any raw roster, machine-watch,
    /// event-store, or member-session lookup.
    ///
    /// This is intentionally the only state-level escape hatch for composite
    /// handlers.  Callers receive the same principal-bound handle after the
    /// admission; they do not receive Owner authority.
    async fn admitted_handle_for(
        &self,
        mob_id: &MobId,
        required: ControlScope,
    ) -> Result<MobHandle, MobError> {
        let handle = self.handle_for(mob_id).await?;
        handle.admit_control_scope(required).await?;
        Ok(handle)
    }

    /// Test seam: an agent mob tool call, or a keyed member send, parks here,
    /// once, between its admission against a mob and its action, so a test
    /// can replace the mob under the id in between. A no-op outside tests.
    pub(crate) async fn pause_agent_call(&self) {
        #[cfg(test)]
        {
            let hold = self
                .agent_call_hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
        }
    }

    /// Chokepoint (b) gate for verbs that never send a MobCommand
    /// (ADJ-P5-11): `mob_create` / `mob_list` are Owner-only in v1 — a
    /// per-mob grant cannot authorize creating a mob that does not exist
    /// yet, and a per-mob List grant cannot authorize cross-mob
    /// enumeration. Typed deny; `presented` is the caller's resolved set
    /// for the (nonexistent / cross-mob) target: the empty set.
    fn require_console_owner(&self, required: ControlScope) -> Result<(), MobError> {
        match &self.console_principal {
            MobControlPrincipal::Owner => Ok(()),
            _ => Err(MobError::ScopeDenied(ScopeDenial {
                required,
                presented: BTreeSet::new(),
            })),
        }
    }

    /// Chokepoint (b) resolver gate for per-mob verbs that bypass the actor
    /// (watch-read projections, event-router subscriptions — DEC-P5E-9).
    /// Owner short-circuits without a clock read (A16); external principals
    /// resolve against the mob's committed machine projection with ONE
    /// wall-clock read per decision. Revocation/expiry take effect at
    /// admission time only: an already-open stream survives a later revoke
    /// (ADJ-P5-17; teardown-on-revoke is a recorded v2 hardening).
    fn require_console_scope(
        &self,
        handle: &MobHandle,
        required: ControlScope,
    ) -> Result<(), MobError> {
        if matches!(self.console_principal, MobControlPrincipal::Owner) {
            return Ok(());
        }
        // Pre-epoch clock fails CLOSED (u64::MAX ⇒ every finite expiry is
        // expired) — a broken clock must never revive expired grants.
        let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(u64::MAX);
        handle
            .resolve_control_policy(now_ms)?
            .require(required)
            .map_err(MobError::from)
    }

    // ─── Control-scope grants (phase 5, §17.2 SD-3) ──────────────────
    //
    // The three wrappers do no scope checking themselves: grant/revoke are
    // gated at chokepoint (a) under AdminGrants, and the grants read
    // self-gates in its actor arm (owner implicit ⇒ zero v1 behavior
    // change).

    pub async fn mob_grant_scopes(
        &self,
        mob_id: &MobId,
        principal: meerkat_core::auth::PrincipalId,
        scopes: BTreeSet<ControlScope>,
        expires_at_ms: Option<u64>,
    ) -> Result<OperatorGrant, MobError> {
        self.handle_for(mob_id)
            .await?
            .grant_scopes(
                self.console_principal.clone(),
                principal,
                scopes,
                expires_at_ms,
            )
            .await
    }

    pub async fn mob_revoke_scopes(
        &self,
        mob_id: &MobId,
        principal: meerkat_core::auth::PrincipalId,
        scopes: Option<BTreeSet<ControlScope>>,
    ) -> Result<bool, MobError> {
        self.handle_for(mob_id)
            .await?
            .revoke_scopes(self.console_principal.clone(), principal, scopes)
            .await
    }

    pub async fn mob_grants(&self, mob_id: &MobId) -> Result<Vec<OperatorGrant>, MobError> {
        self.handle_for(mob_id)
            .await?
            .grants(self.console_principal.clone())
            .await
    }

    // ─── Phase-7 console verbs (§17, DEC-P7A-7 ratified table) ───────
    //
    // The ONE console assembly seam: RPC handlers, the public MCP
    // observation tools, and the CLI verbs all consume exactly these
    // methods (ADJ-P7-3). Actor-routed verbs are gated once at chokepoint
    // (a) against the console principal injected here; the two watch-read
    // projections (`mob_hosts`, `mob_route_installs`) bypass the actor and
    // take the existing chokepoint-(b) resolver gate under `List`.

    /// By-identity member transcript page (SD-1). THE single
    /// `MobMemberHistoryResult` envelope-assembly point (ADJ-P7-3):
    /// placement + provenance come verbatim from the actor's placement
    /// switch (the ADJ-P7-2 domain enrichment) — local pages are
    /// `placement: None` + `ControllingHostVerified`, bridge-served pages
    /// are `placement: Some(host)` + `HostClaimed`. No surface re-derives
    /// them. `ReadHistory`-gated at chokepoint (a).
    pub async fn mob_member_history(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        from_index: Option<u64>,
        limit: Option<u32>,
    ) -> Result<meerkat_contracts::wire::MobMemberHistoryResult, MobError> {
        let domain = self
            .handle_for(mob_id)
            .await?
            .member_history(self.console_principal.clone(), identity, from_index, limit)
            .await?;
        Ok(meerkat_contracts::wire::MobMemberHistoryResult {
            page: domain.page,
            generation: domain.generation,
            placement: domain
                .placement
                .map(|host| meerkat_contracts::wire::WireHostRef(host.as_str().to_string())),
            provenance: domain.provenance,
        })
    }

    /// Force-cancel's HARD sibling (DEC-P6E-8). `Cancel`-gated at
    /// chokepoint (a).
    pub async fn mob_hard_cancel_member(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        reason: String,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .hard_cancel_member(self.console_principal.clone(), identity, reason)
            .await
    }

    /// Run-fenced Stop of one exact member run, as the console principal.
    /// A stale run id is the typed `NotCurrent` receipt.
    pub async fn mob_stop_member_run(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        run_id: meerkat_core::lifecycle::RunId,
        reason: String,
    ) -> Result<meerkat_contracts::WireRunStopReceipt, MobError> {
        self.handle_for(mob_id)
            .await?
            .stop_member_run(self.console_principal.clone(), identity, run_id, reason)
            .await
    }

    /// Open a live realtime channel on a member (§16.4). The returned
    /// [`meerkat_contracts::wire::LiveOpenResult`] carries the owning
    /// host's URL + single-use token VERBATIM (DEC-P6B-C7/C8): never
    /// logged, Debug-formatted, or retained here. `Live`-gated at
    /// chokepoint (a).
    pub async fn mob_member_live_open(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        turning_mode: Option<meerkat_contracts::wire::RealtimeTurningMode>,
        transport: Option<meerkat_contracts::wire::LiveOpenTransport>,
    ) -> Result<meerkat_contracts::wire::LiveOpenResult, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_open(
                self.console_principal.clone(),
                identity,
                turning_mode,
                transport,
            )
            .await
    }

    /// Close one NAMED live channel (close-what-you-name, DEC-P6B-C9).
    /// `Live`-gated at chokepoint (a).
    pub async fn mob_member_live_close(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: String,
    ) -> Result<meerkat_contracts::wire::LiveCloseStatus, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_close(self.console_principal.clone(), identity, channel_id)
            .await
    }

    /// Live point read; `channel_id: None` is the reply-loss discovery
    /// primitive (ADJ-P6B-2). `Live`-gated at chokepoint (a).
    pub async fn mob_member_live_status(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: Option<String>,
    ) -> Result<meerkat_mob::MemberLiveStatusDomain, MobError> {
        self.handle_for(mob_id)
            .await?
            .member_live_status(self.console_principal.clone(), identity, channel_id)
            .await
    }

    /// Drive one DL10 live control verb (commit_input / interrupt /
    /// truncate / refresh — closed vocabulary). `Live`-gated at
    /// chokepoint (a).
    pub async fn mob_member_live_control(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        channel_id: String,
        verb: meerkat_contracts::wire::supervisor_bridge::BridgeLiveControlVerb,
    ) -> Result<meerkat_contracts::wire::supervisor_bridge::BridgeLiveControlOutcome, MobError>
    {
        self.handle_for(mob_id)
            .await?
            .member_live_control(self.console_principal.clone(), identity, channel_id, verb)
            .await
    }

    /// Bind a member-host daemon (§7.2 step 2). Ambient authority comes
    /// from the `handle_for` rebind; the actor arm gates `AdminHost` at
    /// chokepoint (a).
    pub async fn mob_bind_host(
        &self,
        mob_id: &MobId,
        request: meerkat_mob::HostBindRequest,
    ) -> Result<meerkat_mob::HostBindReport, MobError> {
        self.handle_for(mob_id).await?.bind_host(request).await
    }

    /// Revoke a bound (or bind-requested) member host. `AdminHost`-gated
    /// at chokepoint (a); the typed report names the released
    /// materializations (DEC-P7B-14 — never fabricated surface-side).
    pub async fn mob_revoke_host(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<meerkat_mob::HostRevokeReport, MobError> {
        self.handle_for(mob_id).await?.revoke_host(host_id).await
    }

    /// Host roster projection (SD-5). Watch-read that bypasses the actor
    /// ⇒ chokepoint-(b) resolver gate under `List` (the ADJ-P5-12
    /// status-read classification extended to the phase-7 projections).
    pub async fn mob_hosts(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_contracts::wire::MobHostsResult, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.hosts()
    }

    /// Route-install obligation projection (ADJ-P4-9a). Watch-read ⇒
    /// chokepoint-(b) `List` gate. The drain (`drive_route_installs`)
    /// stays surface-less on EVERY surface (DEC-P7B-16).
    pub async fn mob_route_installs(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_contracts::wire::MobRouteInstallsResult, MobError> {
        let handle = self.handle_for(mob_id).await?;
        self.require_console_scope(&handle, ControlScope::List)?;
        handle.route_installs().await
    }

    // ─── Realm profile CRUD ──────────────────────────────────────────

    fn require_realm_profile_store(
        &self,
    ) -> Result<&Arc<dyn meerkat_mob::RealmProfileStore>, meerkat_mob::MobError> {
        self.realm_profile_store_selection.store().ok_or_else(|| {
            meerkat_mob::MobError::Internal("realm profile store not configured".to_string())
        })
    }

    /// Create a new realm profile.
    pub async fn realm_profile_create(
        &self,
        name: &str,
        profile: &meerkat_mob::Profile,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        // Realm-wide mutation has no target mob whose grants could authorize
        // it. Keep the v1 cross-mob posture identical to mob_create.
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .create(name, profile)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Get a realm profile by name.
    pub async fn realm_profile_get(
        &self,
        name: &str,
    ) -> Result<Option<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::List)?;
        let store = self.require_realm_profile_store()?;
        store
            .get(name)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// List all realm profiles.
    pub async fn realm_profile_list(
        &self,
    ) -> Result<Vec<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::List)?;
        let store = self.require_realm_profile_store()?;
        store
            .list()
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Update a realm profile with CAS revision.
    pub async fn realm_profile_update(
        &self,
        name: &str,
        profile: &meerkat_mob::Profile,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .update(name, profile, expected_revision)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Delete a realm profile with CAS revision.
    pub async fn realm_profile_delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobError> {
        self.require_console_owner(ControlScope::SendCommand)?;
        let store = self.require_realm_profile_store()?;
        store
            .delete(name, expected_revision)
            .await
            .map_err(|e| meerkat_mob::MobError::Internal(e.to_string()))
    }

    /// Create MCP state backed by an in-memory local session service.
    ///
    /// This deterministic dev/test service simulates turns and comms; it is
    /// not an LLM-backed service or an external peer transport.
    ///
    /// Mints `Owner` internally: the in-memory dev/test console is a local
    /// single-user surface (A16 posture, DEC-P5E-8).
    pub fn new_in_memory() -> Arc<Self> {
        Self::new_in_memory_as(MobControlPrincipal::Owner)
    }

    /// [`Self::new_in_memory`] serving an explicit console principal — the
    /// production principal-binding seam (ADJ-P5-10) with an in-memory
    /// session service. A `MobControlPrincipal::External` console makes
    /// every mob verb subject to that principal's ControlScope grants; this
    /// is the deterministic non-owner lane the scope-matrix rows drive and
    /// the byte-identical path v2 bearer auth lands on.
    pub fn new_in_memory_as(console_principal: MobControlPrincipal) -> Arc<Self> {
        let service = Arc::new(LocalSessionService::new());
        let runtime_adapter = Some(Arc::clone(&service.runtime_adapter));
        Self::from_acquired_runtime_adapter(service, runtime_adapter, console_principal)
            .into_shared()
    }
}

struct LocalCommsRuntime {
    name: String,
    peer_id: PeerId,
    public_key_bytes: [u8; 32],
    address: String,
    key: String,
    trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
    trusted_descriptors: RwLock<
        HashMap<String, HashMap<GeneratedCommsTrustAuthoritySourceKind, TrustedPeerDescriptor>>,
    >,
    private_trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
    meerkat_machine_trust_owner:
        std::sync::RwLock<Option<meerkat_core::comms::GeneratedPeerCommsOwnerToken>>,
    mob_machine_trust_owner: RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
    notify: Arc<tokio::sync::Notify>,
}

fn register_live_actor(
    registry: &meerkat_session::LiveSessionActorRegistry,
    slot: &meerkat_session::LiveSessionActorWitnessSlot,
    session_id: SessionId,
) -> Result<meerkat_session::LiveSessionActorWitness, SessionError> {
    registry.insert_and_publish(
        slot,
        session_id,
        meerkat_core::TransientTurnContextStateHandle::new(),
    )
}

fn begin_live_actor_materialization(
    bindings: Option<&meerkat_core::SessionRuntimeBindings>,
) -> Result<Option<meerkat_runtime::RuntimeActorMaterializationPermit>, SessionError> {
    let Some(bindings) = bindings else {
        return Ok(None);
    };
    match meerkat_runtime::begin_session_runtime_actor_materialization(bindings) {
        Ok(permit) => Ok(Some(permit)),
        Err(meerkat_runtime::RuntimeActorMaterializationError::RegistrationClosed) => {
            Err(SessionError::NotFound {
                id: bindings.session_id().clone(),
            })
        }
        Err(meerkat_runtime::RuntimeActorMaterializationError::InvalidAuthority(reason)) => Err(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(reason)),
        ),
    }
}

async fn commit_live_actor_materialization_or_discard<S>(
    service: &S,
    permit: Option<meerkat_runtime::RuntimeActorMaterializationPermit>,
    actor_witness: &meerkat_session::LiveSessionActorWitness,
) -> Result<(), SessionError>
where
    S: MobSessionService + ?Sized,
{
    let Some(permit) = permit else {
        return Ok(());
    };
    if let Err(error) = permit.commit() {
        let cleanup = service
            .discard_live_session_actor_under_runtime_turn_boundary(actor_witness)
            .await;
        return Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(match cleanup {
                Ok(_) => format!(
                    "runtime actor materialization commit failed for session {}: {error}",
                    actor_witness.session_id()
                ),
                Err(cleanup_error) => format!(
                    "runtime actor materialization commit failed for session {}: {error}; exact actor cleanup also failed: {cleanup_error}",
                    actor_witness.session_id()
                ),
            }),
        ));
    }
    Ok(())
}

fn encode_ed25519_public_key(bytes: &[u8; 32]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity("ed25519:".len() + 44);
    encoded.push_str("ed25519:");
    for chunk in bytes.chunks_exact(3) {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        encoded.push(TABLE[(n & 0x3f) as usize] as char);
    }
    let remainder = bytes.chunks_exact(3).remainder();
    if !remainder.is_empty() {
        let b0 = remainder[0];
        let b1 = remainder.get(1).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8);
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if remainder.len() == 2 {
            encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            encoded.push('=');
        } else {
            encoded.push('=');
            encoded.push('=');
        }
    }
    encoded
}

impl LocalCommsRuntime {
    fn new(name: &str) -> Self {
        // SHA-256 of the name: deterministic and collision-resistant. A byte
        // fold collided for sibling member names, and MobMachine then refused
        // the second member's peer registration for reusing a peer id.
        let public_key_bytes: [u8; 32] = {
            use sha2::Digest as _;
            sha2::Sha256::digest(name.as_bytes()).into()
        };
        let peer_id = PeerId::from_ed25519_pubkey(&public_key_bytes);
        Self {
            name: name.to_string(),
            peer_id,
            public_key_bytes,
            address: format!("inproc://{name}"),
            key: encode_ed25519_public_key(&public_key_bytes),
            trusted: RwLock::new(HashMap::new()),
            trusted_descriptors: RwLock::new(HashMap::new()),
            private_trusted: RwLock::new(HashMap::new()),
            meerkat_machine_trust_owner: std::sync::RwLock::new(None),
            mob_machine_trust_owner: RwLock::new(None),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    async fn validate_generated_trust_authority_owner(
        &self,
        authority: &meerkat_core::comms::CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        let expected_meerkat = self
            .meerkat_machine_trust_owner
            .read()
            .map_err(|_| {
                SendError::Validation("poisoned meerkat_machine_trust_owner lock".to_string())
            })?
            .clone();
        let expected_mob = self.mob_machine_trust_owner.read().await.clone();
        authority
            .validate_target_source_owner_token(expected_meerkat.as_ref(), expected_mob.as_ref())
            .map_err(SendError::Validation)
    }

    async fn add_generated_trust_source(
        &self,
        peer: TrustedPeerDescriptor,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
        private: bool,
    ) -> Result<bool, SendError> {
        let peer_id = peer.peer_id.as_str().to_string();
        let mut trusted = self.trusted.write().await;
        let mut descriptors = self.trusted_descriptors.write().await;
        let mut private_trusted = self.private_trusted.write().await;

        let source_exists = trusted
            .get(&peer_id)
            .is_some_and(|sources| sources.contains(&source_kind));
        let existing_descriptor = descriptors
            .get(&peer_id)
            .and_then(|by_source| by_source.get(&source_kind));
        let private_source_exists = private_trusted
            .get(&peer_id)
            .is_some_and(|sources| sources.contains(&source_kind));

        match (source_exists, existing_descriptor) {
            (true, Some(existing)) if existing == &peer && private_source_exists == private => {
                return Ok(false);
            }
            (false, None) if !private_source_exists => {}
            _ => {
                return Err(SendError::Validation(format!(
                    "generated trust source {source_kind:?} for {peer_id} already owns different trust material"
                )));
            }
        }

        descriptors
            .entry(peer_id.clone())
            .or_default()
            .insert(source_kind, peer);
        let created = trusted
            .entry(peer_id.clone())
            .or_default()
            .insert(source_kind);
        if private {
            private_trusted
                .entry(peer_id)
                .or_default()
                .insert(source_kind);
        }
        Ok(created)
    }

    async fn remove_generated_trust_source(
        &self,
        peer_id: &str,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> bool {
        let mut trusted = self.trusted.write().await;
        let mut descriptors = self.trusted_descriptors.write().await;
        let mut private_trusted = self.private_trusted.write().await;
        let removed = remove_trust_source(&mut trusted, peer_id, source_kind);
        if !removed {
            return false;
        }
        if let Some(by_source) = descriptors.get_mut(peer_id) {
            by_source.remove(&source_kind);
            if by_source.is_empty() {
                descriptors.remove(peer_id);
            }
        }
        if let Some(sources) = private_trusted.get_mut(peer_id) {
            sources.remove(&source_kind);
            if sources.is_empty() {
                private_trusted.remove(peer_id);
            }
        }
        true
    }
}

impl meerkat_core::handles::PeerCommsInstallTarget for LocalCommsRuntime {
    fn install_generated_peer_comms_handle(
        &self,
        install: meerkat_core::handles::GeneratedPeerCommsInstall,
    ) -> Result<(), String> {
        let target_peer_id = self.generated_peer_comms_target_endpoint()?.peer_id;
        if install.target_peer_id() != target_peer_id {
            return Err(format!(
                "generated peer-comms install targets peer_id {} but runtime peer_id is {}",
                install.target_peer_id(),
                target_peer_id
            ));
        }
        let owner_token = install.owner_token();
        let mut expected = self
            .meerkat_machine_trust_owner
            .write()
            .map_err(|_| "poisoned meerkat_machine_trust_owner lock".to_string())?;
        if let Some(existing) = expected.as_ref()
            && !existing.same_owner(&owner_token)
        {
            return Err(
                "target runtime is already bound to a different generated MeerkatMachine trust owner"
                    .to_string(),
            );
        }
        *expected = Some(owner_token);
        Ok(())
    }
}

fn remove_trust_source(
    trusted: &mut HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>,
    peer_id: &str,
    source_kind: GeneratedCommsTrustAuthoritySourceKind,
) -> bool {
    let Some(sources) = trusted.get_mut(peer_id) else {
        return false;
    };
    if !sources.remove(&source_kind) {
        return false;
    }
    if sources.is_empty() {
        trusted.remove(peer_id);
    }
    true
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreCommsRuntime for LocalCommsRuntime {
    fn peer_id(&self) -> Option<PeerId> {
        Some(self.peer_id)
    }

    fn public_key(&self) -> Option<String> {
        Some(self.key.clone())
    }

    fn public_key_bytes(&self) -> Option<[u8; 32]> {
        Some(self.public_key_bytes)
    }

    fn comms_name(&self) -> Option<String> {
        Some(self.name.clone())
    }

    fn advertised_address(&self) -> Option<String> {
        Some(self.address.clone())
    }

    async fn apply_trust_mutation(
        &self,
        mutation: CommsTrustMutation,
    ) -> Result<CommsTrustMutationResult, SendError> {
        match mutation {
            CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                authority
                    .validate_public_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                let created = self
                    .add_generated_trust_source(peer, authority.trust_row_owner_kind(), false)
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                let parsed_peer_id = PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_public_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                    .await;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
            CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                authority
                    .validate_private_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                let created = self
                    .add_generated_trust_source(peer, authority.trust_row_owner_kind(), true)
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)
                    .await?;
                let parsed_peer_id = PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_private_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                    .await;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
        }
    }

    async fn trusted_peer_projection_snapshot_for_source(
        &self,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
        let descriptors = self.trusted_descriptors.read().await;
        let private_trusted = self.private_trusted.read().await;
        let mut peers = descriptors
            .iter()
            .filter_map(|(peer_id, by_source)| {
                if private_trusted
                    .get(peer_id)
                    .is_some_and(|sources| sources.contains(&source_kind))
                {
                    return None;
                }
                by_source.get(&source_kind).cloned()
            })
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.peer_id.as_str().cmp(&right.peer_id.as_str()));
        Ok(peers)
    }

    async fn install_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write().await;
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    async fn validate_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let expected = self.mob_machine_trust_owner.read().await;
        if let Some(existing) = expected.as_ref()
            && !Arc::ptr_eq(existing, &owner)
        {
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        Ok(())
    }

    async fn install_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write().await;
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    async fn add_private_trusted_peer(
        &self,
        _peer: TrustedPeerDescriptor,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "add_private_trusted_peer requires apply_trust_mutation authority".to_string(),
        ))
    }

    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        Ok(match cmd {
            CommsCommand::PeerLifecycle { .. } => SendReceipt::PeerLifecycleSent {
                envelope_id: uuid::Uuid::new_v4(),
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
            },
            _ => SendReceipt::InputAccepted {
                interaction_id: InteractionId(uuid::Uuid::nil()),
                stream_reserved: false,
            },
        })
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.notify.clone()
    }
}

struct LocalSessionActor {
    comms: Arc<LocalCommsRuntime>,
    /// The mob the session was built for, from its typed member binding.
    mob: Option<MobId>,
    witness: meerkat_session::LiveSessionActorWitness,
    // The lock covers sequence allocation and broadcast send together, so
    // concurrent publishers cannot send allocated sequences out of order.
    #[allow(
        clippy::mutex_integer,
        reason = "serializes allocation and publication"
    )]
    next_event_seq: std::sync::Mutex<u64>,
}

struct LocalSessionService {
    sessions: RwLock<HashMap<SessionId, LocalSessionActor>>,
    actor_registry: meerkat_session::LiveSessionActorRegistry,
    archived_views: RwLock<HashMap<SessionId, SessionView>>,
    system_messages: RwLock<HashMap<SessionId, Vec<AppendSystemContextRequest>>>,
    /// Per-session broadcast channels for event streaming.
    event_txs:
        RwLock<HashMap<SessionId, tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    counter: std::sync::atomic::AtomicU64,
    archive_delay_ms: std::sync::atomic::AtomicU64,
    archive_failures: Arc<InMemoryArchiveFailureControl>,
    /// The spawner restrictions this service handed to the build of each
    /// session it created, kept past archive so a successor can keep them.
    tool_restrictions: std::sync::Mutex<
        HashMap<SessionId, Result<meerkat_core::PredecessorToolRestrictions, String>>,
    >,
}

impl LocalSessionService {
    fn new() -> Self {
        Self::new_with_archive_failures(Arc::new(InMemoryArchiveFailureControl::default()))
    }

    /// [`Self::new`] over a runtime the test configured first (a governed
    /// one). Tests only.
    #[cfg(test)]
    fn new_over_runtime_for_tests(runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        Self {
            runtime_adapter,
            ..Self::new()
        }
    }

    fn new_with_archive_failures(archive_failures: Arc<InMemoryArchiveFailureControl>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
            archived_views: RwLock::new(HashMap::new()),
            system_messages: RwLock::new(HashMap::new()),
            event_txs: RwLock::new(HashMap::new()),
            runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            counter: std::sync::atomic::AtomicU64::new(0),
            archive_delay_ms: std::sync::atomic::AtomicU64::new(0),
            archive_failures,
            tool_restrictions: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn set_archive_delay_ms(&self, delay_ms: u64) {
        self.archive_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }

    async fn retire_with_machine_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        if !self.sessions.read().await.contains_key(session_id)
            && !self.runtime_adapter.contains_session(session_id).await
        {
            return Ok(());
        }

        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        match meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
            .await
        {
            Ok(_) => Ok(()),
            Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
                self.runtime_adapter
                    .register_session(session_id.clone())
                    .await
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive register before retire failed: {error}"),
                        ))
                    })?;
                meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive retire failed after registration: {error}"),
                        ))
                    })
            }
            Err(error) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "machine archive retire failed: {error}"
                )),
            )),
        }
    }

    async fn create_session_with_actor_slot(
        &self,
        req: CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        let build = req.build;
        let sid = build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        let mob = build
            .as_ref()
            .and_then(|build| build.mob_member_binding.as_ref())
            .map(|binding| MobId::from(binding.mob_id.as_str()));
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = build
            .as_ref()
            .and_then(|build| build.comms_name.clone())
            .unwrap_or_else(|| format!("session-{n}"));
        let provided_bindings = match build.as_ref().map(|build| &build.runtime_build_mode) {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                if bindings.session_id() != &sid {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "machine-prepared session bindings for {} do not match created session {}",
                            bindings.session_id(),
                            sid
                        )),
                    ));
                }
                Some(bindings.clone())
            }
            Some(meerkat_core::RuntimeBuildMode::StandaloneEphemeral) | None => None,
        };
        self.runtime_adapter
            .register_session(sid.clone())
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "machine register session failed: {error}"
                )))
            })?;
        let machine_prepared = provided_bindings.is_some();
        let bindings = match provided_bindings {
            Some(bindings) => bindings,
            None => self
                .runtime_adapter
                .prepare_bindings(sid.clone())
                .await
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine prepare bindings failed: {error}"
                    )))
                })?,
        };
        let actor_materialization_permit =
            begin_live_actor_materialization(machine_prepared.then_some(&bindings))?;
        let comms = Arc::new(LocalCommsRuntime::new(&name));
        bindings
            .install_peer_comms_on(comms.as_ref())
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "machine peer-comms install failed: {error}"
                )))
            })?;

        let actor_witness = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&sid) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live session actor is already registered: {sid}"
                    )),
                ));
            }
            let witness =
                register_live_actor(&self.actor_registry, actor_witness_slot, sid.clone())?;
            sessions.insert(
                sid.clone(),
                LocalSessionActor {
                    comms,
                    mob,
                    witness: witness.clone(),
                    next_event_seq: std::sync::Mutex::new(1),
                },
            );
            self.tool_restrictions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    sid.clone(),
                    meerkat_core::PredecessorToolRestrictions::from_build_request(build.as_ref())
                        .map_err(|error| error.to_string()),
                );
            witness
        };
        commit_live_actor_materialization_or_discard(
            self,
            actor_materialization_permit,
            &actor_witness,
        )
        .await?;
        self.system_messages
            .write()
            .await
            .insert(sid.clone(), Vec::new());
        let (tx, _) = tokio::sync::broadcast::channel::<EventEnvelope<AgentEvent>>(256);
        self.event_txs.write().await.insert(sid.clone(), tx);
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: sid,
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
            run_usage: None,
            request_usage: Vec::new(),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for LocalSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        self.create_session_with_actor_slot(req, &actor_witness_slot)
            .await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let sessions = self.sessions.read().await;
        let actor = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        // System messages are transcript entries, not request-local prompt
        // decoration. This lightweight local service retains their identity
        // separately and keeps the conversational prompt byte-exact.
        let effective_prompt = req.prompt.clone();

        let event_tx = self.event_txs.read().await.get(id).cloned();
        let next_seq = |seq: &mut u64| {
            let current = *seq;
            *seq += 1;
            current
        };
        if let Some(event_tx) = event_tx {
            let mut seq = actor
                .next_event_seq
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunStarted {
                    request_reasoning: None,
                    identity: Default::default(),
                    session_id: id.clone(),
                    input: meerkat_core::types::RunInput::Content {
                        content: effective_prompt.clone(),
                    },
                },
            ));
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnStarted {
                    turn_number: 1,
                    assistant_message_id: None,
                },
            ));
            let usage = Usage::default();
            let turn_usage = usage.clone();
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: Some(meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Other,
                        "mob-mcp-test-agent",
                        turn_usage,
                    )),
                    assistant_message_id: None,
                },
            ));
            let _ = event_tx.send(EventEnvelope::new_session(
                id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunCompleted {
                    identity: Default::default(),
                    session_id: id.clone(),
                    result: "ok".to_string(),
                    structured_output: None,
                    extraction_required: false,
                    usage: usage.into(),
                    terminal_cause_kind: None,
                    assistant_message_id: None,
                },
            ));
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: id.clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
            run_usage: None,
            request_usage: Vec::new(),
        })
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(())
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return self
                .archived_views
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                model: "claude-sonnet-4-5".to_string(),
                provider: Provider::Anthropic,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(self
            .sessions
            .read()
            .await
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(reason) = self.archive_failures.failure_for(id).await {
            return Err(SessionError::Unsupported(reason));
        }
        let archive_delay_ms = self
            .archive_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if archive_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(archive_delay_ms)).await;
        }
        let removed = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(id) && !self.actor_registry.remove_current(id) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry omitted current session {id} during archive"
                    )),
                ));
            }
            sessions.remove(id)
        };
        self.event_txs.write().await.remove(id);
        if removed.is_some() {
            let message_count = self
                .system_messages
                .read()
                .await
                .get(id)
                .map_or(0, std::vec::Vec::len);
            self.archived_views.write().await.insert(
                id.clone(),
                SessionView {
                    state: SessionInfo {
                        session_id: id.clone(),
                        created_at: SystemTime::now(),
                        updated_at: SystemTime::now(),
                        message_count,
                        is_active: false,
                        model: "claude-sonnet-4-5".to_string(),
                        provider: Provider::Anthropic,
                        last_assistant_text: None,
                        labels: Default::default(),
                    },
                    billing: SessionUsage {
                        total_tokens: 0,
                        usage: Usage::default(),
                    },
                },
            );
            Ok(())
        } else {
            Err(SessionError::NotFound { id: id.clone() })
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceCommsExt for LocalSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|actor| actor.comms.clone() as Arc<dyn CoreCommsRuntime>)
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        let actor = sessions.get(session_id)?;
        actor.comms.event_injector()
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        let actor = sessions.get(session_id)?;
        actor.comms.interaction_event_injector()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceControlExt for LocalSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        let mut pending = self.system_messages.write().await;
        let entry = pending.entry(id.clone()).or_default();
        if let Some(key) = req.idempotency_key.as_deref()
            && let Some(existing) = entry
                .iter()
                .find(|existing| existing.idempotency_key.as_deref() == Some(key))
        {
            if existing.content == req.content && existing.source == req.source {
                return Ok(AppendSystemContextResult {
                    status: AppendSystemContextStatus::Duplicate,
                });
            }
            return Err(SessionControlError::Conflict {
                id: id.clone(),
                key: key.to_string(),
            });
        }
        entry.push(req);
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Applied,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceHistoryExt for LocalSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        if self.sessions.read().await.contains_key(id)
            || self.archived_views.read().await.contains_key(id)
        {
            let messages = self
                .system_messages
                .read()
                .await
                .get(id)
                .into_iter()
                .flatten()
                .map(|append| {
                    meerkat_core::Message::System(meerkat_core::SystemMessage::with_identity(
                        append.content.render_text(),
                        append.source.clone(),
                        append.idempotency_key.clone(),
                    ))
                })
                .collect::<Vec<_>>();
            return Ok(SessionHistoryPage::from_messages(
                id.clone(),
                &messages,
                query,
            ));
        }
        Err(SessionError::NotFound { id: id.clone() })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSessionService for LocalSessionService {
    async fn predecessor_tool_restrictions(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::PredecessorToolRestrictions, SessionError> {
        // Only sessions this service created have a record.
        self.tool_restrictions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?
            .map_err(|reason| {
                SessionError::Unsupported(format!(
                    "inherited tool restriction evidence for session {session_id}: {reason}"
                ))
            })
    }

    async fn subscribe_session_activity(
        &self,
        _session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::service::SessionError> {
        // This fixture's sessions never report an active turn.
        Ok(meerkat_mob::MemberSessionActivity::inactive())
    }

    async fn observe_member_status_view(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::MemberStatusSessionView, SessionError> {
        // In-memory sessions: `read` is the published live state.
        meerkat_mob::observe_member_status_view_via_read(self, session_id).await
    }

    async fn observe_live_durable_source(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::LiveDurableSourceObservation, SessionError> {
        // In-memory sessions have no persisted durable source: this service
        // inherits `load_persisted_session -> Ok(None)`, so the live open's
        // durable-source check always rejects. Readiness agrees with the
        // open and never reports a live session as committed. The archived
        // map still distinguishes an archived session; neither read queues
        // behind a session actor.
        Ok(
            if self.archived_views.read().await.contains_key(session_id) {
                meerkat_mob::LiveDurableSourceObservation::Archived
            } else {
                meerkat_mob::LiveDurableSourceObservation::Absent
            },
        )
    }

    async fn publish_boundary_appends_discarded_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), SessionError> {
        if discarded.session_id != *actor_witness.session_id() {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(
                    "boundary discard session does not match the exact actor".to_string(),
                ),
            ));
        }
        // Hold the registry boundary through publication so a predecessor
        // cannot resolve the event channel of a replacement actor.
        let sessions = self.sessions.read().await;
        let actor = sessions
            .get(actor_witness.session_id())
            .filter(|actor| actor.witness.eq(actor_witness) && actor_witness.is_live())
            .ok_or_else(|| SessionError::NotFound {
                id: actor_witness.session_id().clone(),
            })?;
        if discarded.input_ids.is_empty() {
            return Ok(());
        }
        let event_txs = self.event_txs.read().await;
        let event_tx =
            event_txs
                .get(actor_witness.session_id())
                .ok_or_else(|| SessionError::NotFound {
                    id: actor_witness.session_id().clone(),
                })?;
        let mut next_seq = actor
            .next_event_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let event = EventEnvelope::new_session(
            actor_witness.session_id().clone(),
            *next_seq,
            None,
            AgentEvent::BoundaryAppendsDiscarded(discarded.clone()),
        );
        *next_seq += 1;
        let _ = event_tx.send(event);
        Ok(())
    }

    async fn fork_persisted_session_at_turn_boundary(
        &self,
        _source_session_id: &meerkat_core::SessionId,
        _message_count: Option<usize>,
        _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        _target: meerkat_core::DurableSessionForkTarget,
        _bound: std::time::Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, meerkat_core::service::SessionError> {
        Err(meerkat_core::service::SessionError::Unsupported(
            "in-memory local session service has no durable fork authority".to_string(),
        ))
    }

    async fn commit_live_delegation_final_transcript(
        &self,
        _machine: &meerkat_runtime::MeerkatMachine,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        Err(SessionError::Unsupported(
            "live delegation canonical projection requires a runtime-backed persistent session service"
                .into(),
        ))
    }

    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        _machine: &meerkat_runtime::MeerkatMachine,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
        _bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        Err(SessionError::Unsupported(
            "live delegation canonical projection requires a runtime-backed persistent session service".into(),
        ))
    }

    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
        meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
    }

    async fn observe_session_resume_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
        // This service owns process-local maps, not durable RuntimeStore rows.
        Ok(meerkat_mob::SessionResumeAuthority::default())
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        <Self as SessionService>::create_session(self, req).await
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        _session_id: &SessionId,
        _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "in-memory local session service has no store-owned boundary authority".to_string(),
        ))
    }

    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        _session_id: &SessionId,
        _runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        Ok(0)
    }

    /// In-memory service: nothing durable survives archive, so the two-read
    /// composition is the exact truth — `ArchivedNotRevivable` cannot exist.
    async fn load_session_for_resume(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::runtime::ResumeSessionLoad, SessionError> {
        use meerkat_mob::runtime::ResumeSessionLoad;
        if let Some(session) = self.load_persisted_session(session_id).await? {
            return Ok(ResumeSessionLoad::Active(Box::new(session)));
        }
        if let Some(session) = self.load_revivable_retired_session(session_id).await? {
            return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
        }
        Ok(ResumeSessionLoad::Absent)
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
        _resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_actor_slot(req, actor_witness_slot)
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.archive_with_mob_lifecycle_authority(session_id).await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        _deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        // MemberSessionDisposalArc transfers this process-local archive into
        // its owned task before waiting on the caller's absolute deadline.
        // Declare that deadline contract explicitly instead of inheriting the
        // fail-closed Unsupported default.
        self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session(session_id).await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        let removed = {
            let mut sessions = self.sessions.write().await;
            let Some(actor) = sessions.get(witness.session_id()) else {
                return Ok(false);
            };
            if !actor.witness.eq(witness) {
                return Ok(false);
            }
            if !self.actor_registry.compare_remove(witness) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry rejected current witness for {}",
                        witness.session_id()
                    )),
                ));
            }
            sessions.remove(witness.session_id()).is_some()
        };
        if removed {
            self.system_messages
                .write()
                .await
                .remove(witness.session_id());
            self.event_txs.write().await.remove(witness.session_id());
        }
        Ok(removed)
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        let txs = self.event_txs.read().await;
        let tx = txs
            .get(session_id)
            .ok_or_else(|| StreamError::NotFound(format!("session {session_id}")))?;
        let rx = tx.subscribe();
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        })))
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(self.actor_registry.contains(session_id))
    }

    fn supports_runtime_turn_apply(&self) -> bool {
        true
    }

    fn acquire_runtime_adapter(
        &self,
        explicit: Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Result<
        Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>>,
        meerkat_runtime::RuntimeDriverError,
    > {
        let owner = Some(Arc::clone(&self.runtime_adapter));
        if let (Some(owner), Some(requested)) = (owner.as_ref(), explicit.as_ref())
            && !owner.shares_runtime_execution_owner_with(requested)
        {
            return Err(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                },
            );
        }
        Ok(owner.or(explicit))
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.retire_with_machine_archive_authority(session_id)
            .await?;
        SessionService::archive(self, session_id).await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(session_id) && !self.actor_registry.remove_current(session_id)
            {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "live actor registry omitted current session {session_id} during discard"
                    )),
                ));
            }
            sessions.remove(session_id);
        }
        self.system_messages.write().await.remove(session_id);
        self.event_txs.write().await.remove(session_id);
        Ok(())
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: meerkat_core::service::StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        let run_result = <Self as SessionService>::start_turn(self, session_id, req).await?;
        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary,
                    contributing_input_ids,
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                run_result,
            ),
        )
    }

    /// A live session belongs to the mob its typed member binding named when
    /// it was built. Several mobs share this service: a session is never
    /// reported as another mob's, and one built without a member binding is
    /// no mob's.
    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.sessions
            .read()
            .await
            .get(session_id)
            .and_then(|actor| actor.mob.as_ref())
            .is_some_and(|owner| owner == mob_id)
    }
}

impl MobMcpState {
    pub fn new_in_memory_with_archive_delay(delay_ms: u64) -> Arc<Self> {
        let session_service = Arc::new(LocalSessionService::new());
        session_service.set_archive_delay_ms(delay_ms);
        let runtime_adapter = Some(Arc::clone(&session_service.runtime_adapter));
        Arc::new(Self::from_acquired_runtime_adapter(
            session_service,
            runtime_adapter,
            MobControlPrincipal::Owner,
        ))
    }

    #[doc(hidden)]
    pub fn new_in_memory_with_archive_failure_control()
    -> (Arc<Self>, Arc<InMemoryArchiveFailureControl>) {
        let failures = Arc::new(InMemoryArchiveFailureControl::default());
        let session_service = Arc::new(LocalSessionService::new_with_archive_failures(
            failures.clone(),
        ));
        let runtime_adapter = Some(Arc::clone(&session_service.runtime_adapter));
        (
            Arc::new(Self::from_acquired_runtime_adapter(
                session_service,
                runtime_adapter,
                MobControlPrincipal::Owner,
            )),
            failures,
        )
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

pub struct MobMcpDispatcher {
    state: Arc<MobMcpState>,
    tools: Arc<[Arc<ToolDef>]>,
    ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
    owner_bridge_session_id: Option<SessionId>,
}

impl MobMcpDispatcher {
    pub fn new(state: Arc<MobMcpState>) -> Self {
        state.start_workgraph_flow_reconciler();
        const PRIMER: &str = "A mob is a managed multi-agent team with shared lifecycle/events: \
            create -> spawn members -> wire/unwire trust -> stop/resume -> complete/destroy.";
        const COMMON: &str = "Use real mob tools (no simulation). Keep and reuse the returned \
            mob_id from mob_create. All mob_* tools except mob_create and mob_list require mob_id.";
        let tools = vec![
            // ── Mob-level (mob_*) ──────────────────────────────────────
            tool(
                "mob_create",
                &format!("{PRIMER} Create a new mob from a definition. Returns mob_id."),
                json!({"type":"object","properties":{"definition":{"type":"object"}},"required":["definition"]}),
            ),
            tool(
                "mob_list",
                &format!("List mobs or get detail for one. Omit mob_id for summary of all mobs; \
                     provide mob_id for detailed status of that mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}}}),
            ),
            tool(
                "mob_lifecycle",
                &format!("Lifecycle action on a mob. action: stop | resume | reset | complete | destroy. {COMMON}"),
                lifecycle_input_schema(),
            ),
            tool(
                "mob_events",
                &format!("Fetch mob lifecycle events. Optional: after_cursor, limit. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"after_cursor":{"type":"integer"},"limit":{"type":"integer"}},"required":["mob_id"]}),
            ),
            tool(
                "mob_run_flow",
                &format!("Start a configured flow run. Required: mob_id, flow_id. Optional params object. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"flow_id":{"type":"string"},"params":{"type":"object"}},"required":["mob_id","flow_id"]}),
            ),
            tool(
                "mob_flow_status",
                &format!("Read flow run status and ledgers by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            tool(
                "mob_cancel_flow",
                &format!("Cancel an in-flight flow run by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            // ── Member-level (mob_* member ops) ────────────────────────
            tool(
                "mob_spawn_member",
                &format!("Spawn one or more mob members. Required: mob_id, specs[].profile, specs[].agent_identity. \
                     Optional per-spec: backend=session|external, runtime_mode=autonomous_host|turn_driven, \
                     initial_message, labels (key-value map), context (opaque JSON), \
                     placement (comms peer id of a bound member host). {COMMON}"),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "specs":{
                            "type":"array",
                            "items":{
                                "type":"object",
                                "properties":{
                                    "profile":{"type":"string"},
                                    "agent_identity":{"type":"string"},
                                    "initial_message": content_input_schema(),
                                    "backend":{"type":"string","enum":["session","external"]},
                                    "binding": runtime_binding_schema(),
                                    "runtime_mode":{"type":"string","enum":["autonomous_host","turn_driven"]},
                                    "labels":{"type":"object","additionalProperties":{"type":"string"}},
                                    "context":{"type":"object"},
                                    "placement":{"type":"string","description":"Comms peer id of a bound member host to place this member on (multi-host mobs)"}
                                },
                                "required":["profile","agent_identity"]
                            }
                        }
                    },
                    "required":["mob_id","specs"]
                }),
            ),
            tool(
                "mob_retire_member",
                &format!("Retire a spawned mob member by identity. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_list_members",
                &format!("List current members in a mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}},"required":["mob_id"]}),
            ),
            tool(
                "mob_wire",
	                &format!("Wire or unwire bidirectional trust between a local member and a peer target. \
	                     action: wire | unwire. Use external_binding for wire and external name handles for unwire. {COMMON}"),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "agent_identity":{"type":"string"},
                        "peer":{
                            "oneOf":[
                                {
                                    "type":"object",
                                    "properties":{"local":{"type":"string"}},
                                    "required":["local"],
                                    "additionalProperties":false
                                },
                                {
                                    "type":"object",
                                    "properties":{
	                                        "external_binding":{
	                                            "type":"object",
	                                            "properties":{
	                                                "name":{"type":"string"},
                                                "address":{"type":"string"},
                                                "identity":{
                                                    "type":"object",
                                                    "properties":{
                                                        "kind":{"type":"string","enum":["ed25519_public_key"]},
                                                        "public_key":{"type":"string"}
                                                    },
                                                    "required":["kind","public_key"],
                                                    "additionalProperties":false
                                                }
	                                            },
	                                            "required":["name","address","identity"],
	                                            "additionalProperties":false
	                                        }
	                                    },
	                                    "required":["external_binding"],
	                                    "additionalProperties":false
	                                },
	                                {
	                                    "type":"object",
	                                    "properties":{
	                                        "external":{
	                                            "type":"object",
	                                            "properties":{
	                                                "name":{"type":"string"}
	                                            },
	                                            "required":["name"],
	                                            "additionalProperties":false
	                                        }
	                                    },
	                                    "required":["external"],
	                                    "additionalProperties":false
	                                }
	                            ]
	                        },
                        "action":{"type":"string","enum":["wire","unwire"]}
                    },
                    "required":["mob_id","agent_identity","peer","action"]
                }),
            ),
            tool(
                "mob_respawn",
                &format!("Retire and re-spawn a member with the same profile. \
                     Required: mob_id, agent_identity. Optional: initial_message. \
                     Returns an identity-native respawn receipt. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"},"initial_message": content_input_schema()},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_force_cancel",
                &format!("Force-cancel a member's in-flight turn. Unlike retire, this \
                     interrupts immediately without graceful shutdown. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_member_status",
                &format!("Get execution status snapshot for a member. Returns status, \
                     output_preview (the current bridge session's last committed assistant text), \
                     tokens_used, and is_final. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"agent_identity":{"type":"string"}},"required":["mob_id","agent_identity"]}),
            ),
            tool(
                "mob_wait_kickoff",
                &format!(
                    "Wait until autonomous kickoff turns complete. Optional: member_ids (subset), timeout_ms. Returns member snapshots. {COMMON}"
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "member_ids":{"type":"array","items":{"type":"string"}},
                        "timeout_ms":{"type":"integer","minimum":1}
                    },
                    "required":["mob_id"]
                }),
            ),
            tool(
                "mob_wait_ready",
                &format!(
                    "Wait until mob startup readiness (members bound but kickoff not required). Optional: member_ids (subset), timeout_ms. Returns member snapshots; in session-bound agent turns this may detach and return status plus operation_id. {COMMON}"
                ),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "member_ids":{"type":"array","items":{"type":"string"}},
                        "timeout_ms":{"type":"integer","minimum":1}
                    },
                    "required":["mob_id"]
                }),
            ),
        ]
        .into();
        Self {
            state,
            tools,
            ops_registry: None,
            owner_bridge_session_id: None,
        }
    }

    fn dispatch_detached_wait_ready(
        &self,
        call: ToolCallView<'_>,
        mob_id: MobId,
        member_ids: Option<Vec<AgentIdentity>>,
        timeout_ms: Option<u64>,
    ) -> Option<Result<meerkat_core::ToolDispatchOutcome, ToolError>> {
        let registry = self.ops_registry.as_ref()?.clone();
        let owner_bridge_session_id = self.owner_bridge_session_id.clone()?;
        let operation_id = OperationId::new();
        let spec = OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: owner_bridge_session_id,
            display_name: format!("mob_wait_ready {}", mob_id.as_str()),
            source_label: "mob_wait_ready".to_string(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        };

        if let Err(error) = registry.register_operation(spec) {
            return Some(Err(ToolError::execution_failed(format!(
                "register mob_wait_ready background operation: {error}"
            ))));
        }
        if let Err(error) = registry.provisioning_succeeded(&operation_id) {
            return Some(Err(ToolError::execution_failed(format!(
                "start mob_wait_ready background operation: {error}"
            ))));
        }

        let state = Arc::clone(&self.state);
        let registry_for_task = Arc::clone(&registry);
        let operation_id_for_task = operation_id.clone();
        let mob_id_for_task = mob_id.clone();
        let tasks = Arc::clone(&self.state);
        let admitted = tasks.state_tasks.spawn(
            state_tasks::StateTaskKind::Owned,
            "detached mob_wait_ready",
            async move {
                let started = Instant::now();
                match state
                    .mob_wait_ready(&mob_id_for_task, member_ids, timeout_ms)
                    .await
                {
                    Ok(members) => {
                        let content = match serde_json::to_string(&json!({ "members": members })) {
                            Ok(content) => content,
                            Err(error) => {
                                let _ = registry_for_task.fail_operation(
                                    &operation_id_for_task,
                                    format!("encode mob_wait_ready background result: {error}"),
                                );
                                return;
                            }
                        };
                        let duration_ms =
                            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                        let result = OperationResult {
                            id: operation_id_for_task.clone(),
                            content,
                            is_error: false,
                            duration_ms,
                            tokens_used: 0,
                        };
                        let _ =
                            registry_for_task.complete_operation(&operation_id_for_task, result);
                    }
                    Err(error) => {
                        let _ = registry_for_task
                            .fail_operation(&operation_id_for_task, error.to_string());
                    }
                }
            },
        );
        if admitted.is_none() {
            let error = MobMcpState::stood_down_error();
            let _ = registry.fail_operation(&operation_id, error.to_string());
            return Some(Err(ToolError::execution_failed(error.to_string())));
        }

        Some(Self::encode_detached_wait_result(
            call,
            mob_id,
            operation_id,
        ))
    }

    fn encode_detached_wait_result(
        call: ToolCallView<'_>,
        mob_id: MobId,
        operation_id: OperationId,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let content = serde_json::to_string(&json!({
            "status": "waiting",
            "mob_id": mob_id,
            "operation_id": operation_id,
            "wait_policy": "detached"
        }))
        .map_err(|error| ToolError::execution_failed(format!("encode tool result: {error}")))?;
        Ok(meerkat_core::ToolDispatchOutcome::new(
            ToolResult::new(call.id.to_string(), content, false),
            vec![AsyncOpRef::detached(operation_id)],
            vec![],
        ))
    }
}

fn tool(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: name.into(),
        description: description.to_string(),
        input_schema,
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mob,
            source_id: "mob".into(),
            artifact: None,
        }),
    })
}

fn lifecycle_input_schema() -> serde_json::Value {
    // K1: ONE infallible schema generator — no fail-open null-schema arm.
    meerkat_core::schema::tool_input_schema_for::<MobLifecycleParams>()
}

fn content_input_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            { "type": "string" },
            {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "text" },
                                "text": { "type": "string" }
                            },
                            "required": ["type", "text"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "data"]
                        }
                    ]
                }
            }
        ]
    })
}

fn runtime_binding_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "session" }
                },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "external" },
                    "address": { "type": "string" },
                    "bootstrap_token": { "type": "string" },
                    "identity": {
                        "type": "object",
                        "properties": {
                            "kind": { "const": "ed25519_public_key" },
                            "public_key": { "type": "string" }
                        },
                        "required": ["kind", "public_key"],
                        "additionalProperties": false
                    }
                },
                "required": ["kind", "address", "identity"],
                "additionalProperties": false
            }
        ]
    })
}

fn encode(call: ToolCallView<'_>, payload: serde_json::Value) -> Result<ToolResult, ToolError> {
    let content = serde_json::to_string(&payload)
        .map_err(|e| ToolError::execution_failed(format!("encode result: {e}")))?;
    Ok(ToolResult::new(call.id.to_string(), content, false))
}

fn map_mob_err(call: ToolCallView<'_>, err: MobError) -> ToolError {
    let message = format!("tool '{}' failed: {err}", call.name);
    match err.structured_data() {
        Some(data) => ToolError::execution_failed_with_data(message, data),
        None => ToolError::execution_failed(message),
    }
}

fn map_mob_respawn_err(call: ToolCallView<'_>, err: meerkat_mob::MobRespawnError) -> ToolError {
    let message = format!("tool '{}' failed: {err}", call.name);
    match err.structured_data() {
        Some(data) => ToolError::execution_failed_with_data(message, data),
        None => ToolError::execution_failed(message),
    }
}

fn map_destroy_err(call: ToolCallView<'_>, err: MobMcpDestroyError) -> ToolError {
    match err {
        MobMcpDestroyError::Incomplete { report } => ToolError::execution_failed_with_data(
            format!(
                "tool '{}' failed: mob destroy incomplete: {}",
                call.name,
                destroy_report_summary(&report)
            ),
            MobMcpDestroyError::incomplete_error_data(&report),
        ),
        MobMcpDestroyError::Mob(error) => map_mob_err(call, error),
    }
}

#[derive(Deserialize)]
struct MobCreateArgs {
    definition: MobDefinitionInput,
}
#[derive(Deserialize)]
struct MobListArgs {
    #[serde(default)]
    mob_id: Option<String>,
}
#[derive(Deserialize)]
struct MobIdArgs {
    mob_id: String,
}
#[derive(Debug, Deserialize)]
struct MobSpawnMeerkatArgs {
    profile: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    binding: Option<meerkat_contracts::WireRuntimeBinding>,
    #[serde(default)]
    runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    context: Option<serde_json::Value>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
    /// Host placement (comms peer id of a bound member host, multi-host
    /// mobs ADJ-7); admission stays with the spawn-exec ladder.
    #[serde(default)]
    placement: Option<String>,
}
#[derive(Debug, Deserialize)]
struct SpawnManyMeerkatsArgs {
    mob_id: String,
    specs: Vec<MobSpawnMeerkatArgs>,
}
#[derive(Deserialize)]
struct RetireArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct WireActionArgs {
    mob_id: String,
    agent_identity: String,
    peer: WireActionPeerTarget,
    action: WireMobWireAction,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireActionPeerTarget {
    Local(WireActionLocalPeerTarget),
    ExternalBinding(WireActionExternalBindingTarget),
    External(WireActionExternalHandleTarget),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionLocalPeerTarget {
    local: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalBindingTarget {
    external_binding: WireActionExternalBinding,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalHandleTarget {
    external: WireActionExternalHandle,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalBinding {
    name: String,
    address: String,
    identity: meerkat_contracts::WireTrustedPeerIdentity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActionExternalHandle {
    name: String,
}

fn runtime_binding_from_wire(
    binding: meerkat_contracts::WireRuntimeBinding,
) -> Result<meerkat_mob::RuntimeBinding, String> {
    match binding {
        meerkat_contracts::WireRuntimeBinding::Session => Ok(meerkat_mob::RuntimeBinding::Session),
        meerkat_contracts::WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity.resolve().map_err(|err| err.to_string())?;
            Ok(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

/// Project the canonical mob lifecycle state into its wire mirror. Keeps the
/// surface emitting a closed typed status rather than the bare `MobState` text.
///
/// Exported so sibling surfaces (e.g. the JSON-RPC `mob/list` / `mob/status`
/// handlers) consume this single `MobState -> WireMobLifecycleStatus` owner
/// rather than re-deriving the projection from `MobState::to_string()`.
pub fn wire_mob_lifecycle_status(state: MobState) -> WireMobLifecycleStatus {
    match state {
        MobState::Creating => WireMobLifecycleStatus::Creating,
        MobState::Running => WireMobLifecycleStatus::Running,
        MobState::Stopped => WireMobLifecycleStatus::Stopped,
        MobState::Completed => WireMobLifecycleStatus::Completed,
        MobState::Destroyed => WireMobLifecycleStatus::Destroyed,
    }
}

/// Why a listed mob's actor has stopped, on the wire: the fields a failed
/// `mob/spawn_many` row carries, from the same typed error.
pub fn wire_mob_lifecycle_failure(error: &MobError) -> meerkat_contracts::WireMobLifecycleFailure {
    meerkat_contracts::WireMobLifecycleFailure {
        cause: meerkat_mob::mob_error_wire_code(error),
        message: error.to_string(),
        code: error.wire_error_code(),
        structured_data: error.structured_data(),
    }
}

/// One listed mob on the wire: its last machine phase and, once its actor
/// has stopped, why. Every surface that lists mobs projects through this.
pub fn wire_mob_status_row(
    mob_id: &MobId,
    observation: &meerkat_mob::MobLifecycleObservation,
) -> meerkat_contracts::MobStatusResult {
    meerkat_contracts::MobStatusResult {
        mob_id: mob_id.to_string(),
        status: wire_mob_lifecycle_status(observation.phase),
        failure: observation
            .failure
            .as_deref()
            .map(wire_mob_lifecycle_failure),
    }
}

impl WireActionArgs {
    fn resolve(
        self,
    ) -> Result<
        (
            String,
            AgentIdentity,
            meerkat_mob::PeerTarget,
            WireMobWireAction,
        ),
        String,
    > {
        let action = self.action;
        let target = match self.peer {
            WireActionPeerTarget::Local(WireActionLocalPeerTarget { local }) => {
                meerkat_mob::PeerTarget::Local(local.into())
            }
            WireActionPeerTarget::ExternalBinding(WireActionExternalBindingTarget {
                external_binding,
            }) => {
                meerkat_mob::PeerTarget::ExternalBinding(meerkat_mob::ExternalPeerBindingSpec::new(
                    external_binding.name,
                    external_binding.address,
                    external_binding.identity,
                ))
            }
            WireActionPeerTarget::External(WireActionExternalHandleTarget { external }) => {
                if matches!(action, WireMobWireAction::Wire) {
                    return Err("wire external peer requires external_binding".to_string());
                }
                let peer_name = PeerName::new(external.name)
                    .map_err(|e| format!("invalid external peer name: {e}"))?;
                meerkat_mob::PeerTarget::ExternalName(peer_name)
            }
        };
        Ok((
            self.mob_id,
            AgentIdentity::from(self.agent_identity),
            target,
            action,
        ))
    }
}
#[derive(Deserialize)]
struct RunFlowArgs {
    mob_id: String,
    flow_id: String,
    #[serde(default)]
    params: serde_json::Value,
}
#[derive(Deserialize)]
struct FlowStatusArgs {
    mob_id: String,
    run_id: String,
}
#[derive(Deserialize)]
struct EventsArgs {
    mob_id: String,
    #[serde(default)]
    after_cursor: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize)]
struct RespawnArgs {
    mob_id: String,
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<WireContentInput>,
}
#[derive(Deserialize)]
struct ForceCancelArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct MeerkatStatusArgs {
    mob_id: String,
    agent_identity: String,
}
#[derive(Deserialize)]
struct WaitKickoffArgs {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
#[derive(Deserialize)]
struct WaitReadyArgs {
    mob_id: String,
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
fn default_limit() -> usize {
    100
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for MobMcpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let started = Instant::now();
        tracing::info!(
            target: "mob_tools",
            "MobMcpDispatcher::dispatch start tool={} tool_use_id={}",
            call.name,
            call.id
        );
        let result = match call.name {
            // ── Mob-level ──────────────────────────────────────────────
            "mob_create" => {
                let args: MobCreateArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let definition = agent_input::decode_agent_mob_definition(args.definition)
                    .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                let mob_id = self
                    .state
                    .mob_create_definition(definition)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"mob_id": mob_id}))
            }
            "mob_list" => {
                let args: MobListArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                if let Some(mob_id) = args.mob_id {
                    let status = self
                        .state
                        .mob_status(&MobId::from(mob_id))
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(call, json!({"status": wire_mob_lifecycle_status(status)}))
                } else {
                    let mobs = self
                        .state
                        .mob_list()
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(
                        call,
                        json!({"mobs": mobs.iter().map(|(id, observation)| wire_mob_status_row(id, observation)).collect::<Vec<_>>() }),
                    )
                }
            }
            "mob_lifecycle" => {
                let args: MobLifecycleParams = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                // MCP tool surface treats destroy as "() on success"; the
                // structured MobDestroyReport is available via public/RPC
                // lifecycle result envelopes for consumers that need detail.
                let _destroy_report = self
                    .state
                    .mob_lifecycle_action(&mob_id, args.action)
                    .await
                    .map_err(|e| map_destroy_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_events" => {
                let args: EventsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let events = self
                    .state
                    .mob_events(&MobId::from(args.mob_id), args.after_cursor, args.limit)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"events": events}))
            }
            "mob_run_flow" => {
                let args: RunFlowArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = self
                    .state
                    .mob_run_flow(
                        &MobId::from(args.mob_id),
                        FlowId::from(args.flow_id),
                        args.params,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"run_id": run_id}))
            }
            "mob_flow_status" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                let run = self
                    .state
                    .mob_flow_status(&mob_id, run_id.clone())
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let run = meerkat_mob::MobRun::public_flow_status_run_value(run.as_ref())
                    .map_err(|e| map_mob_err(call, e))?;
                let execution_binding = self
                    .state
                    .workgraph_execution_binding_for_flow_run(&mob_id, &run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?
                    .as_ref()
                    .map(workgraph_execution_binding_view)
                    .transpose()
                    .map_err(|e| map_mob_err(call, e))?;
                let result = serde_json::to_value(meerkat_contracts::MobFlowStatusResult {
                    run,
                    execution_binding,
                })
                .map_err(|e| {
                    ToolError::invalid_arguments(
                        call.name,
                        format!("failed to encode flow status: {e}"),
                    )
                })?;
                encode(call, result)
            }
            "mob_cancel_flow" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                self.state
                    .mob_cancel_flow(&MobId::from(args.mob_id), run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            // ── Member-level ───────────────────────────────────────────
            "mob_spawn_member" => {
                let args: SpawnManyMeerkatsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| {
                        let mut s = SpawnMemberSpec::new(spec.profile, spec.agent_identity)
                            .with_creation_source(
                                meerkat_mob::MemberCreationSourceWitness::unavailable(),
                            );
                        s.initial_message = spec
                            .initial_message
                            .map(agent_input::decode_agent_content_input)
                            .transpose()
                            .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                        s.runtime_mode = spec.runtime_mode;
                        s.backend = spec.backend;
                        s.binding = spec
                            .binding
                            .map(runtime_binding_from_wire)
                            .transpose()
                            .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                        s.context = spec.context;
                        s.labels = spec.labels;
                        s.additional_instructions = spec.additional_instructions;
                        s.placement = spec
                            .placement
                            .map(meerkat_mob::machines::mob_machine::HostId::from);
                        Ok(s)
                    })
                    .collect::<Result<Vec<_>, ToolError>>()?;
                let mob_id = MobId::from(args.mob_id);
                let results = self
                    .state
                    .mob_spawn_many(&mob_id, specs)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let results = results
                    .into_iter()
                    .map(
                        |result: Result<
                            meerkat_mob::SpawnResult,
                            meerkat_mob::MobSpawnManyFailure,
                        >| {
                            match result {
                                Ok(spawn_result) => {
                                    let identity = spawn_result.agent_identity.to_string();
                                    json!(MobSpawnManyResultEntry::spawned(
                                        identity.clone(),
                                        WireMemberRef::encode(mob_id.as_str(), &identity),
                                    ))
                                }
                                Err(error) => {
                                    json!(MobSpawnManyResultEntry::failed_with_error_details(
                                        error.cause(),
                                        error.to_string(),
                                        error.error().wire_error_code(),
                                        error.error().structured_data(),
                                    ))
                                }
                            }
                        },
                    )
                    .collect::<Vec<_>>();
                encode(call, json!({"results": results}))
            }
            "mob_retire_member" => {
                let args: RetireArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                self.state
                    .mob_retire(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity),
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_list_members" => {
                let args: MobIdArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let rows: Vec<meerkat_mob::runtime::MobMemberListEntry> = self
                    .state
                    .mob_list_members(&MobId::from(args.mob_id))
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": rows}))
            }
            "mob_wire" => {
                let args: WireActionArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let (mob_id, local, target, action) = args
                    .resolve()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                let mob_id = MobId::from(mob_id);
                match action {
                    WireMobWireAction::Wire => self
                        .state
                        .mob_wire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    WireMobWireAction::Unwire => self
                        .state
                        .mob_unwire(&mob_id, local, target)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                }
                encode(call, json!({"ok": true}))
            }
            "mob_respawn" => {
                let args: RespawnArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let initial_message = args
                    .initial_message
                    .map(agent_input::decode_agent_content_input)
                    .transpose()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e))?;
                match self
                    .state
                    .mob_respawn(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity.as_str()),
                        initial_message,
                    )
                    .await
                {
                    Ok(receipt) => encode(
                        call,
                        json!({
                            "status": WireMobRespawnOutcome::Completed,
                            "receipt": receipt,
                        }),
                    ),
                    Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
                        receipt,
                        failed_peer_ids,
                    }) => encode(
                        call,
                        json!({
                            "status": WireMobRespawnOutcome::TopologyRestoreFailed,
                            "receipt": receipt,
                            "failed_peer_ids": failed_peer_ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
                        }),
                    ),
                    Err(e) => return Err(map_mob_respawn_err(call, e)),
                }
            }
            "mob_force_cancel" => {
                let args: ForceCancelArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                self.state
                    .mob_force_cancel(
                        &MobId::from(args.mob_id),
                        AgentIdentity::from(args.agent_identity),
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "mob_member_status" => {
                let args: MeerkatStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                let identity = AgentIdentity::from(args.agent_identity);
                let snapshot = self
                    .state
                    .mob_member_status(&mob_id, &identity)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                let member_ref =
                    meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), identity.as_str());
                let result = snapshot.to_member_status_result(member_ref).map_err(|e| {
                    ToolError::invalid_arguments(
                        call.name,
                        format!("failed to project mob member status: {e}"),
                    )
                })?;
                encode(call, json!(result))
            }
            "mob_wait_kickoff" => {
                let args: WaitKickoffArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let member_ids = args.member_ids.map(|ids| {
                    ids.into_iter()
                        .map(|id| AgentIdentity::from(id.as_str()))
                        .collect::<Vec<_>>()
                });
                let members = self
                    .state
                    .mob_wait_kickoff(&MobId::from(args.mob_id), member_ids, args.timeout_ms)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": members}))
            }
            "mob_wait_ready" => {
                let args: WaitReadyArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let member_ids = args.member_ids.map(|ids| {
                    ids.into_iter()
                        .map(|id| AgentIdentity::from(id.as_str()))
                        .collect::<Vec<_>>()
                });
                let mob_id = MobId::from(args.mob_id);
                if self.ops_registry.is_some() && self.owner_bridge_session_id.is_some() {
                    self.state
                        .handle_for(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                }
                if let Some(result) = self.dispatch_detached_wait_ready(
                    call,
                    mob_id.clone(),
                    member_ids.clone(),
                    args.timeout_ms,
                ) {
                    return result;
                }
                let members = self
                    .state
                    .mob_wait_ready(&mob_id, member_ids, args.timeout_ms)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": members}))
            }
            _ => Err(ToolError::not_found(call.name)),
        };
        match &result {
            Ok(_) => tracing::info!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch ok tool={} elapsed_ms={}",
                call.name,
                started.elapsed().as_millis()
            ),
            Err(err) => tracing::warn!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch err tool={} elapsed_ms={} err={}",
                call.name,
                started.elapsed().as_millis(),
                err
            ),
        }
        result.map(Into::into)
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        if Arc::strong_count(&self) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        let this = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        Ok(BindOutcome::Bound(Arc::new(Self {
            state: this.state,
            tools: this.tools,
            ops_registry: Some(registry),
            owner_bridge_session_id: Some(owner_bridge_session_id),
        })))
    }
}

pub fn workgraph_execution_binding_view(
    binding: &meerkat::WorkExecutionBinding,
) -> Result<meerkat_contracts::WireWorkGraphFlowExecutionBinding, MobError> {
    let meerkat::WorkExecutionTarget::MobFlow {
        mob_id,
        flow_id,
        flow_config_digest,
        run_id,
        ..
    } = &binding.target;
    use meerkat_contracts::WireWorkExecutionLifecyclePhase as Phase;
    let lifecycle_phase = match binding.machine_state.lifecycle_phase {
        meerkat::WorkExecutionLifecycleState::Absent => Phase::Absent,
        meerkat::WorkExecutionLifecycleState::LaunchRequested => Phase::LaunchRequested,
        meerkat::WorkExecutionLifecycleState::LaunchUncertain => Phase::LaunchUncertain,
        meerkat::WorkExecutionLifecycleState::LaunchQuarantined => Phase::LaunchQuarantined,
        meerkat::WorkExecutionLifecycleState::Running => Phase::Running,
        meerkat::WorkExecutionLifecycleState::EvidenceProjectionRequested => {
            Phase::EvidenceProjectionRequested
        }
        meerkat::WorkExecutionLifecycleState::FailureEvidenceProjectionRequested => {
            Phase::FailureEvidenceProjectionRequested
        }
        meerkat::WorkExecutionLifecycleState::CancellationEvidenceProjectionRequested => {
            Phase::CancellationEvidenceProjectionRequested
        }
        meerkat::WorkExecutionLifecycleState::LaunchFailureEvidenceProjectionRequested => {
            Phase::LaunchFailureEvidenceProjectionRequested
        }
        meerkat::WorkExecutionLifecycleState::WorkClosureRequested => Phase::WorkClosureRequested,
        meerkat::WorkExecutionLifecycleState::FlowFailed => Phase::FlowFailed,
        meerkat::WorkExecutionLifecycleState::FlowCanceled => Phase::FlowCanceled,
        meerkat::WorkExecutionLifecycleState::EvidenceProjected => Phase::EvidenceProjected,
        meerkat::WorkExecutionLifecycleState::WorkClosed => Phase::WorkClosed,
        meerkat::WorkExecutionLifecycleState::LaunchFailed => Phase::LaunchFailed,
    };
    Ok(meerkat_contracts::WireWorkGraphFlowExecutionBinding {
        binding_id: binding.binding_id.to_string(),
        work_ref: meerkat_contracts::WireWorkGraphFlowWorkRef {
            realm_id: binding.work_ref.realm_id.clone(),
            namespace: binding.work_ref.namespace.to_string(),
            item_id: binding.work_ref.item_id.to_string(),
        },
        mob_id: mob_id.to_string(),
        flow_id: flow_id.to_string(),
        flow_config_digest: flow_config_digest.clone(),
        run_id: run_id.to_string(),
        lifecycle_phase,
        binding_revision: binding.machine_state.revision,
        supersedes: binding.supersedes.as_ref().map(ToString::to_string),
        evidence_id: binding.evidence_id(),
        created_at: binding.created_at.to_rfc3339(),
    })
}

#[derive(Debug, Clone)]
pub struct McpToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl McpToolError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn capability_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    pub fn destroy_incomplete(report: &meerkat_mob::MobDestroyReport) -> Self {
        Self {
            code: meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            message: MobMcpDestroyError::incomplete_message(report),
            data: Some(MobMcpDestroyError::incomplete_error_data(report)),
        }
    }

    /// Shared typed-detail envelope (§17.4, DEC-P7B-9): `code` is the
    /// stable `ErrorCode::jsonrpc_code()` (recoverable via
    /// `from_jsonrpc_code`), `data` is the BARE detail struct — the SAME
    /// data shape RPC ships, no MCP-only wrapper. A detail that fails to
    /// serialize fails CLOSED as an internal error (mirrors
    /// `mob_scope_denied_error`), never as a silently detail-less success
    /// shape.
    fn from_wire_detail(
        detail: meerkat_contracts::wire::WireMobErrorDetail,
        message: String,
    ) -> Self {
        match detail.detail_value() {
            Ok(data) => Self {
                code: detail.code().jsonrpc_code(),
                message,
                data: Some(data),
            },
            Err(e) => Self::internal(format!("failed to serialize mob error detail: {e}")),
        }
    }

    /// §17.4 typed rendering for mob errors (DEC-P7B-9): the four console
    /// codes carry stable JSON-RPC codes + typed data via
    /// [`MobError::wire_detail`]; everything else keeps the byte-identical
    /// legacy `invalid_params(err.to_string())` fallback.
    pub fn from_mob(err: &MobError) -> Self {
        match err.wire_detail() {
            Some(detail) => Self::from_wire_detail(detail, err.to_string()),
            None => Self {
                code: err
                    .wire_error_code()
                    .map_or(-32602, meerkat_contracts::ErrorCode::jsonrpc_code),
                message: err.to_string(),
                data: err.structured_data(),
            },
        }
    }

    pub fn from_workgraph(err: &meerkat::WorkGraphError) -> Self {
        match meerkat::WorkGraphMachine::public_error_class(err) {
            Ok(
                meerkat::WorkGraphPublicErrorClass::NotFound
                | meerkat::WorkGraphPublicErrorClass::Conflict
                | meerkat::WorkGraphPublicErrorClass::InvalidTransition
                | meerkat::WorkGraphPublicErrorClass::InvalidArguments,
            ) => Self::invalid_params(err.to_string()),
            Ok(meerkat::WorkGraphPublicErrorClass::CapabilityUnavailable) => {
                Self::capability_unavailable(err.to_string())
            }
            Ok(meerkat::WorkGraphPublicErrorClass::StoreError) | Err(_) => {
                Self::internal(err.to_string())
            }
        }
    }

    pub fn from_workgraph_flow(err: &WorkGraphFlowBridgeError) -> Self {
        match err {
            WorkGraphFlowBridgeError::WorkGraphUnavailable => {
                Self::capability_unavailable(err.to_string())
            }
            WorkGraphFlowBridgeError::WorkGraph(inner) => Self::from_workgraph(inner),
            WorkGraphFlowBridgeError::Mob(inner) => Self::from_mob(inner),
            WorkGraphFlowBridgeError::InvalidBinding(_) => Self::invalid_params(err.to_string()),
            WorkGraphFlowBridgeError::AmbiguousLaunch { binding_id, run_id } => Self {
                code: -32041,
                message: err.to_string(),
                data: Some(json!({
                    "kind": "workgraph_flow_launch_ambiguous",
                    "binding_id": binding_id,
                    "run_id": run_id,
                    "guidance": "reconcile, or explicitly abandon the uncertain launch with its expected binding revision",
                })),
            },
            WorkGraphFlowBridgeError::LaunchFailed { binding_id, detail } => Self {
                code: -32042,
                message: err.to_string(),
                data: Some(json!({
                    "kind": "workgraph_flow_launch_failed",
                    "binding_id": binding_id,
                    "detail": detail,
                    "guidance": "create a new execution binding that supersedes this terminal attempt",
                })),
            },
            WorkGraphFlowBridgeError::RunMissing { binding_id, run_id } => Self {
                code: -32043,
                message: err.to_string(),
                data: Some(json!({
                    "kind": "workgraph_flow_run_missing",
                    "binding_id": binding_id,
                    "run_id": run_id,
                    "guidance": "restore the owning Mob runtime or reconcile the binding through the host recovery surface",
                })),
            },
            WorkGraphFlowBridgeError::ConflictingRun {
                binding_id,
                expected_run_id,
                observed_run_id,
            } => Self {
                code: -32044,
                message: err.to_string(),
                data: Some(json!({
                    "kind": "workgraph_flow_conflicting_run",
                    "binding_id": binding_id,
                    "expected_run_id": expected_run_id,
                    "observed_run_id": observed_run_id,
                    "guidance": "the binding is quarantined; inspect the owning Mob runtime before owner adjudication",
                })),
            },
            WorkGraphFlowBridgeError::LaunchQuarantined { binding_id, detail } => Self {
                code: -32045,
                message: err.to_string(),
                data: Some(json!({
                    "kind": "workgraph_flow_launch_quarantined",
                    "binding_id": binding_id,
                    "detail": detail,
                    "guidance": "inspect and reconcile the exact run; do not retry or supersede this attempt automatically",
                })),
            },
        }
    }

    /// [`Self::from_mob`] over the respawn wrapper (delegates through
    /// `MobRespawnError::wire_detail`).
    pub fn from_mob_respawn(err: &meerkat_mob::MobRespawnError) -> Self {
        match err.wire_detail() {
            Some(detail) => Self::from_wire_detail(detail, err.to_string()),
            None => Self {
                code: -32602,
                message: err.to_string(),
                data: err.structured_data(),
            },
        }
    }

    /// [`Self::from_mob`] over the destroy wrapper: the `Incomplete` arm
    /// keeps its dedicated `destroy_incomplete` envelope verbatim.
    pub fn from_mob_destroy(err: &MobMcpDestroyError) -> Self {
        match err {
            MobMcpDestroyError::Incomplete { report } => Self::destroy_incomplete(report),
            MobMcpDestroyError::Mob(inner) => Self::from_mob(inner),
        }
    }

    /// Preserve mob-family authorization details for the composite
    /// system-context verb while retaining the legacy invalid-params mapping
    /// for genuine session-control failures.
    pub fn from_mob_append_system_context(err: &MobAppendSystemContextError) -> Self {
        match err {
            MobAppendSystemContextError::Mob(inner) => Self::from_mob(inner),
            MobAppendSystemContextError::Session(inner) => Self::invalid_params(inner.to_string()),
        }
    }
}

/// Return the agent-side `mob_*` dispatcher tool inventory.
///
/// This mirrors `MobMcpDispatcher` and is intended for in-session/agent tool
/// composition helpers. Public host surfaces should use `public_tools_list()`
/// instead.
pub fn tools_list() -> Vec<serde_json::Value> {
    let dispatcher = MobMcpDispatcher::new(MobMcpState::new_in_memory());
    dispatcher
        .tools()
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema
            })
        })
        .collect()
}

/// Dispatch an agent-side `mob_*` tool call against `MobMcpDispatcher`.
///
/// This is the internal dispatcher helper for in-session agent tools. Public
/// host MCP surfaces should use `handle_public_tools_call()` instead.
pub async fn handle_tools_call(
    state: &Arc<MobMcpState>,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, McpToolError> {
    let dispatcher = MobMcpDispatcher::new(state.clone());
    let raw = serde_json::value::RawValue::from_string(arguments.to_string()).map_err(|e| {
        McpToolError {
            code: -32602,
            message: format!("invalid arguments: {e}"),
            data: None,
        }
    })?;
    let result = dispatcher
        .dispatch(ToolCallView {
            id: "mcp-tool-call",
            name,
            args: &raw,
        })
        .await
        .map_err(|e| McpToolError {
            code: -32000,
            message: e.to_string(),
            data: e.structured_data(),
        })?;
    let text = result.result.text_content();
    serde_json::from_str(&text).map_err(|e| McpToolError {
        code: -32603,
        message: format!("invalid tool result payload: {e}"),
        data: Some(json!({"raw": text})),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::collapsible_if,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::workgraph_flow::{WorkGraphFlowBridge, WorkGraphFlowHost};

    /// Debug-stack budget for the console observation path.
    ///
    /// Hosts call `mob_handles_snapshot` deep inside request handlers on a
    /// 2 MiB worker stack. At opt-level=0 a poll frame reserves a slot for
    /// every future a function builds inline, even on paths that never run
    /// it. The restore future built in `ensure_restored` and the sweep
    /// futures handed by value to `tokio::spawn` gave this path about 420 KiB
    /// of frames (`mob_handles_snapshot` 167, `ensure_restored` 167,
    /// `schedule_temporary_council_recovery` 86) and overflowed a host's
    /// first console send. With those futures built in their own frames the
    /// path runs in under 32 KiB; 128 KiB leaves room without hiding a
    /// regression of that size.
    #[cfg(not(target_arch = "wasm32"))]
    const MOB_SNAPSHOT_DEBUG_STACK_BUDGET: usize = 128 * 1024;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn mob_handles_snapshot_fits_its_debug_stack_budget() {
        std::thread::Builder::new()
            .name("mob-snapshot-small-stack".into())
            .stack_size(MOB_SNAPSHOT_DEBUG_STACK_BUDGET)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("small-stack runtime");
                runtime.block_on(async {
                    let state = MobMcpState::new_in_memory();
                    state
                        .mob_handles_snapshot()
                        .await
                        .expect("owner snapshot of an empty state");
                });
            })
            .expect("spawn small-stack thread")
            .join()
            .expect("mob_handles_snapshot must fit its debug stack budget");
    }

    use async_trait::async_trait;
    use meerkat_core::InteractionId;
    use meerkat_core::PlainEventSource;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, CommsTrustMutation, CommsTrustMutationResult,
        GeneratedCommsTrustAuthoritySourceKind, PeerId, SendError, SendReceipt,
        TrustedPeerDescriptor,
    };
    use meerkat_core::event::AgentEvent;
    use meerkat_core::event_injector::{
        EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
    };
    use meerkat_core::interaction::PeerInputCandidate;
    use meerkat_core::service::InitialTurnPolicy;
    use meerkat_core::service::SessionService;
    use meerkat_core::service::{
        CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionSummary,
        SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, SessionId, Usage};
    use meerkat_core::{
        PeerMeta, Provider, Session, SessionMetadata, SessionTooling, ToolCategoryOverride,
    };
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;
    use tokio::sync::Notify;
    use tokio::time::{Duration, Instant, sleep};

    const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

    fn external_peer_target(public_key: &str) -> WireActionPeerTarget {
        serde_json::from_value(json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": public_key
                }
            }
        }))
        .expect("external binding target should deserialize")
    }

    #[tokio::test]
    async fn test_local_comms_runtime_trust_requires_mutation_authority() {
        let runtime = LocalCommsRuntime::new("local");
        let mut pubkey = [0u8; 32];
        pubkey[0] = 42;
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey).to_string();
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer",
        )
        .expect("valid peer descriptor");

        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
        let projection_authority = std::sync::Arc::new(std::sync::Mutex::new(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::new(),
        ));
        {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            authority
                .apply_signal(
                    meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                )
                .expect("Initialize signal");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                        "mob-mcp-local-comms-test",
                    ),
                    runtime_epoch_id: None,
                    initial_run_start_holds: std::collections::BTreeSet::new(),
                },
            )
            .expect("RegisterSession input");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
                        "local",
                        runtime.peer_id.to_string(),
                        runtime.address.clone(),
                        runtime.public_key_bytes,
                    ),
                },
            )
            .expect("PublishLocalEndpoint input");
        }
        let wiring_transition = {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 1,
                    endpoints: BTreeSet::from([endpoint.clone()]),
                },
            )
            .expect("ApplyMobPeerOverlay input")
        };
        let wiring_obligation =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &wiring_transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    std::sync::Arc::clone(&projection_authority),
                ),
            )
            .pop()
            .expect("generated wiring obligation");
        let add_authority =
            meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
                &wiring_obligation,
                &endpoint,
            )
            .expect("generated wiring obligation covers peer");
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
            Arc::new(meerkat_runtime::HandleDslAuthority::from_shared(
                Arc::clone(&projection_authority),
            )),
            &runtime,
        )
        .expect("install generated peer-comms owner");

        let added = runtime
            .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer {
                peer: peer.clone(),
                authority: add_authority,
            })
            .await
            .expect("authorized add succeeds");
        assert_eq!(added, CommsTrustMutationResult::Added { created: true });
        assert!(runtime.trusted.read().await.contains_key(&peer_id));

        let unwiring_transition = {
            let mut authority = projection_authority
                .lock()
                .expect("projection authority lock should not be poisoned");
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 2,
                    endpoints: BTreeSet::new(),
                },
            )
            .expect("ApplyMobPeerOverlay remove input")
        };
        let unwiring_obligation =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &unwiring_transition,
                meerkat_runtime::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    std::sync::Arc::clone(&projection_authority),
                ),
            )
            .pop()
            .expect("generated unwiring obligation");

        let removed = runtime
            .apply_trust_mutation(CommsTrustMutation::RemoveTrustedPeer {
                peer_id: peer_id.clone(),
                authority:
                    meerkat_runtime::protocol_comms_trust_reconcile::removal_authority_for_peer_id(
                        &unwiring_obligation,
                        &peer_id,
                    )
                    .expect("generated unwiring obligation covers peer"),
            })
            .await
            .expect("authorized remove succeeds");
        assert_eq!(removed, CommsTrustMutationResult::Removed { removed: true });
        assert!(!runtime.trusted.read().await.contains_key(&peer_id));
    }

    #[tokio::test]
    async fn test_local_comms_runtime_generated_source_rejects_descriptor_rewrite() {
        let runtime = LocalCommsRuntime::new("local");
        let mut pubkey = [0u8; 32];
        pubkey[0] = 45;
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey).to_string();
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer",
        )
        .expect("valid peer descriptor");
        let rewritten = TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer-renamed".to_string(),
            peer_id.clone(),
            pubkey,
            "inproc://peer-renamed",
        )
        .expect("valid peer descriptor");

        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let created = runtime
            .add_generated_trust_source(peer.clone(), source_kind, false)
            .await
            .expect("initial generated trust row");
        assert!(created);
        let repeated = runtime
            .add_generated_trust_source(peer, source_kind, false)
            .await
            .expect("same generated trust row should be idempotent");
        assert!(!repeated);

        let conflict = runtime
            .add_generated_trust_source(rewritten, source_kind, false)
            .await
            .expect_err("same generated source must not rewrite descriptor material");
        assert!(
            matches!(conflict, SendError::Validation(ref message) if message.contains("already owns different trust material")),
            "unexpected conflict error: {conflict:?}"
        );
        let descriptors = runtime.trusted_descriptors.read().await;
        let descriptor = descriptors
            .get(&peer_id)
            .and_then(|by_source| by_source.get(&source_kind))
            .expect("generated descriptor remains installed");
        assert_eq!(descriptor.name.as_str(), "peer");
        assert_eq!(descriptor.address.to_string(), "inproc://peer");
    }

    #[tokio::test]
    async fn test_local_comms_runtime_source_snapshot_excludes_private_trust() {
        let runtime = LocalCommsRuntime::new("local");
        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let public_key = [46u8; 32];
        let private_key = [47u8; 32];
        let public_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "public-peer".to_string(),
            PeerId::from_ed25519_pubkey(&public_key).to_string(),
            public_key,
            "inproc://public-peer",
        )
        .expect("valid public peer descriptor");
        let private_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "private-peer".to_string(),
            PeerId::from_ed25519_pubkey(&private_key).to_string(),
            private_key,
            "inproc://private-peer",
        )
        .expect("valid private peer descriptor");

        runtime
            .add_generated_trust_source(public_peer.clone(), source_kind, false)
            .await
            .expect("install public generated trust row");
        runtime
            .add_generated_trust_source(private_peer, source_kind, true)
            .await
            .expect("install private generated trust row");

        let snapshot = runtime
            .trusted_peer_projection_snapshot_for_source(source_kind)
            .await
            .expect("read public source projection");
        assert_eq!(snapshot, vec![public_peer]);
    }

    #[tokio::test]
    async fn test_local_comms_runtime_source_removal_is_cancellation_safe() {
        let runtime = Arc::new(LocalCommsRuntime::new("local"));
        let source_kind = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;
        let public_key = [48u8; 32];
        let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            "public-peer".to_string(),
            PeerId::from_ed25519_pubkey(&public_key).to_string(),
            public_key,
            "inproc://public-peer",
        )
        .expect("valid public peer descriptor");
        let peer_id = peer.peer_id.as_str().to_string();
        runtime
            .add_generated_trust_source(peer.clone(), source_kind, false)
            .await
            .expect("install generated trust row");

        let descriptor_guard = runtime.trusted_descriptors.write().await;
        let removal = {
            let runtime = Arc::clone(&runtime);
            let peer_id = peer_id.clone();
            tokio::spawn(async move {
                runtime
                    .remove_generated_trust_source(&peer_id, source_kind)
                    .await
            })
        };

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if tokio::time::timeout(Duration::from_millis(10), runtime.trusted.read())
                .await
                .is_err()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "removal should acquire the membership guard before waiting for descriptors"
            );
            tokio::task::yield_now().await;
        }
        removal.abort();
        let _ = removal.await;
        drop(descriptor_guard);

        assert!(
            runtime.trusted.read().await.contains_key(&peer_id),
            "cancellation before all trust guards are acquired must leave membership intact"
        );
        assert!(
            !runtime
                .add_generated_trust_source(peer.clone(), source_kind, false)
                .await
                .expect("the retained row should remain idempotent"),
            "cancellation must not leave an orphan descriptor that rejects the original row"
        );
        assert_eq!(
            runtime
                .trusted_peer_projection_snapshot_for_source(source_kind)
                .await
                .expect("read public source projection"),
            vec![peer]
        );
    }

    #[test]
    fn mob_spawn_member_args_accept_canonical_external_runtime_binding() {
        let args = serde_json::from_value::<SpawnManyMeerkatsArgs>(json!({
            "mob_id": "mob",
            "specs": [{
                "profile": "worker",
                "agent_identity": "w-ext",
                "binding": {
                    "kind": "external",
                    "address": "inproc://external-worker",
                    "identity": {
                        "kind": "ed25519_public_key",
                        "public_key": ED25519_PUBLIC_KEY_7
                    }
                }
            }]
        }))
        .expect("canonical external runtime binding should deserialize");

        let binding = args
            .specs
            .into_iter()
            .next()
            .and_then(|spec| spec.binding)
            .expect("binding present");
        let resolved = runtime_binding_from_wire(binding)
            .expect("canonical external runtime binding resolves");
        let meerkat_mob::RuntimeBinding::External {
            peer_id, pubkey, ..
        } = resolved
        else {
            panic!("expected external runtime binding");
        };
        let expected_pubkey = [7u8; 32];
        assert_eq!(
            peer_id,
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&expected_pubkey).to_string()
        );
        assert_eq!(pubkey, expected_pubkey);
    }

    #[test]
    fn mob_spawn_member_args_reject_raw_external_runtime_binding_atoms() {
        let err = serde_json::from_value::<SpawnManyMeerkatsArgs>(json!({
            "mob_id": "mob",
            "specs": [{
                "profile": "worker",
                "agent_identity": "w-ext",
                "binding": {
                    "kind": "external",
                    "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                    "address": "inproc://external-worker",
                    "pubkey": vec![7u8; 32]
                }
            }]
        }))
        .expect_err("raw peer_id/pubkey external runtime binding shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_raw_external_peer_atoms() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "external": {
                "name": "external-worker",
                "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                "address": "inproc://external-worker",
                "pubkey": vec![7u8; 32]
            }
        }))
        .expect_err("raw peer_id/pubkey external peer shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("external")
                || msg.contains("external_binding")
                || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_missing_external_binding_pubkey_material() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key"
                }
            }
        }))
        .expect_err("missing external binding pubkey material must fail closed");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity") || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_ambiguous_peer_target_shape() {
        let err = serde_json::from_value::<WireActionPeerTarget>(json!({
            "local": "worker-a",
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": ED25519_PUBLIC_KEY_7
                }
            }
        }))
        .expect_err("legacy wire action must not accept multiple peer target shapes");

        let msg = err.to_string();
        assert!(
            msg.contains("did not match")
                || msg.contains("unknown field")
                || msg.contains("external_binding"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn wire_action_args_rejects_external_handle_for_wire_action() {
        let err = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: serde_json::from_value(json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
            action: WireMobWireAction::Wire,
        }
        .resolve()
        .expect_err("wire action must require external_binding");

        assert!(
            err.contains("external_binding"),
            "expected external_binding validation error, got: {err}"
        );
    }

    #[test]
    fn wire_action_args_passes_external_binding_to_mob_authority() {
        let resolved = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: external_peer_target(ED25519_PUBLIC_KEY_7),
            action: WireMobWireAction::Wire,
        }
        .resolve()
        .expect("canonical external peer binding should resolve as request");

        let (_mob_id, _local, target, _action) = resolved;
        let meerkat_mob::PeerTarget::ExternalBinding(binding) = target else {
            panic!("canonical external peer should remain mob-resolved external binding");
        };
        assert_eq!(binding.name, "external-worker");
        assert_eq!(binding.address, "inproc://external-worker");
    }

    #[test]
    fn wire_action_args_unwire_accepts_external_name_handle() {
        let resolved = WireActionArgs {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            peer: serde_json::from_value(json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
            action: WireMobWireAction::Unwire,
        }
        .resolve()
        .expect("external handle should be valid for unwire");

        let (_mob_id, _local, target, _action) = resolved;
        let meerkat_mob::PeerTarget::ExternalName(peer_name) = target else {
            panic!("unwire external should use the external peer handle");
        };
        assert_eq!(peer_name.as_str(), "external-worker");
    }

    struct MockComms {
        name: String,
        peer_id: meerkat_core::comms::PeerId,
        public_key_bytes: [u8; 32],
        address: String,
        key: String,
        trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
        trusted_descriptors: RwLock<
            HashMap<String, HashMap<GeneratedCommsTrustAuthoritySourceKind, TrustedPeerDescriptor>>,
        >,
        private_trusted: RwLock<HashMap<String, BTreeSet<GeneratedCommsTrustAuthoritySourceKind>>>,
        meerkat_machine_trust_owner:
            std::sync::RwLock<Option<meerkat_core::comms::GeneratedPeerCommsOwnerToken>>,
        mob_machine_trust_owner: RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
        notify: Arc<Notify>,
    }

    struct MockInjector;

    impl EventInjector for MockInjector {
        fn inject(
            &self,
            _body: ContentInput,
            _source: PlainEventSource,
            _handling_mode: HandlingMode,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            Ok(())
        }
    }

    impl SubscribableInjector for MockInjector {
        fn inject_with_delivery_identity(
            &self,
            _input_identity: meerkat_core::service::StartTurnInputIdentity,
            _objective_id: Option<meerkat_core::interaction::ObjectiveId>,
            _content: ContentInput,
            _source: PlainEventSource,
            _handling_mode: HandlingMode,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            Err(EventInjectorError::Closed)
        }

        fn inject_with_subscription(
            &self,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<InteractionSubscription, EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)?;
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let interaction_id = InteractionId(uuid::Uuid::new_v4());
            let interaction_id_for_task = interaction_id;
            tokio::spawn(async move {
                let _ = tx
                    .send(AgentEvent::InteractionComplete {
                        interaction_id: interaction_id_for_task,
                        result: "ok".to_string(),
                        structured_output: None,
                    })
                    .await;
            });
            Ok(InteractionSubscription {
                id: interaction_id,
                events: rx,
            })
        }

        fn inject_with_interaction_id(
            &self,
            _interaction_id: InteractionId,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)
        }
    }

    impl MockComms {
        fn new(name: &str) -> Self {
            // SHA-256 of the name: deterministic and collision-resistant (a
            // byte fold collided for sibling names).
            let public_key_bytes: [u8; 32] = {
                use sha2::Digest as _;
                sha2::Sha256::digest(name.as_bytes()).into()
            };
            let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&public_key_bytes);
            Self {
                name: name.to_string(),
                peer_id,
                public_key_bytes,
                address: format!("inproc://{name}"),
                key: super::encode_ed25519_public_key(&public_key_bytes),
                trusted: RwLock::new(HashMap::new()),
                trusted_descriptors: RwLock::new(HashMap::new()),
                private_trusted: RwLock::new(HashMap::new()),
                meerkat_machine_trust_owner: std::sync::RwLock::new(None),
                mob_machine_trust_owner: RwLock::new(None),
                notify: Arc::new(Notify::new()),
            }
        }

        async fn validate_generated_trust_authority_owner(
            &self,
            authority: &meerkat_core::comms::CommsTrustMutationAuthority,
        ) -> Result<(), SendError> {
            let expected_meerkat = self
                .meerkat_machine_trust_owner
                .read()
                .map_err(|_| {
                    SendError::Validation("poisoned meerkat_machine_trust_owner lock".to_string())
                })?
                .clone();
            let expected_mob = self.mob_machine_trust_owner.read().await.clone();
            authority
                .validate_target_source_owner_token(
                    expected_meerkat.as_ref(),
                    expected_mob.as_ref(),
                )
                .map_err(SendError::Validation)
        }

        async fn add_generated_trust_source(
            &self,
            peer: TrustedPeerDescriptor,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
            private: bool,
        ) -> Result<bool, SendError> {
            let peer_id = peer.peer_id.as_str().to_string();
            let mut trusted = self.trusted.write().await;
            let mut descriptors = self.trusted_descriptors.write().await;
            let mut private_trusted = self.private_trusted.write().await;
            let source_exists = trusted
                .get(&peer_id)
                .is_some_and(|sources| sources.contains(&source_kind));
            let existing_descriptor = descriptors
                .get(&peer_id)
                .and_then(|by_source| by_source.get(&source_kind));
            let private_source_exists = private_trusted
                .get(&peer_id)
                .is_some_and(|sources| sources.contains(&source_kind));
            match (source_exists, existing_descriptor) {
                (true, Some(existing)) if existing == &peer && private_source_exists == private => {
                    return Ok(false);
                }
                (false, None) if !private_source_exists => {}
                _ => {
                    return Err(SendError::Validation(format!(
                        "generated trust source {source_kind:?} for {peer_id} already owns different trust material"
                    )));
                }
            }
            descriptors
                .entry(peer_id.clone())
                .or_default()
                .insert(source_kind, peer);
            let created = trusted
                .entry(peer_id.clone())
                .or_default()
                .insert(source_kind);
            if private {
                private_trusted
                    .entry(peer_id)
                    .or_default()
                    .insert(source_kind);
            }
            Ok(created)
        }

        async fn remove_generated_trust_source(
            &self,
            peer_id: &str,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
        ) -> bool {
            let mut trusted = self.trusted.write().await;
            let mut descriptors = self.trusted_descriptors.write().await;
            let mut private_trusted = self.private_trusted.write().await;
            if !remove_trust_source(&mut trusted, peer_id, source_kind) {
                return false;
            }
            if let Some(by_source) = descriptors.get_mut(peer_id) {
                by_source.remove(&source_kind);
                if by_source.is_empty() {
                    descriptors.remove(peer_id);
                }
            }
            if let Some(sources) = private_trusted.get_mut(peer_id) {
                sources.remove(&source_kind);
                if sources.is_empty() {
                    private_trusted.remove(peer_id);
                }
            }
            true
        }
    }

    impl meerkat_core::handles::PeerCommsInstallTarget for MockComms {
        fn install_generated_peer_comms_handle(
            &self,
            install: meerkat_core::handles::GeneratedPeerCommsInstall,
        ) -> Result<(), String> {
            let target_peer_id = self.generated_peer_comms_target_endpoint()?.peer_id;
            if install.target_peer_id() != target_peer_id {
                return Err(format!(
                    "generated peer-comms install targets peer_id {} but runtime peer_id is {}",
                    install.target_peer_id(),
                    target_peer_id
                ));
            }
            let owner_token = install.owner_token();
            let mut expected = self
                .meerkat_machine_trust_owner
                .write()
                .map_err(|_| "poisoned meerkat_machine_trust_owner lock".to_string())?;
            if let Some(existing) = expected.as_ref()
                && !existing.same_owner(&owner_token)
            {
                return Err(
                    "target runtime is already bound to a different generated MeerkatMachine trust owner"
                        .to_string(),
                );
            }
            *expected = Some(owner_token);
            Ok(())
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for MockComms {
        fn peer_id(&self) -> Option<meerkat_core::comms::PeerId> {
            Some(self.peer_id)
        }

        fn public_key(&self) -> Option<String> {
            Some(self.key.clone())
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            Some(self.public_key_bytes)
        }

        fn comms_name(&self) -> Option<String> {
            Some(self.name.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(self.address.clone())
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                    self.validate_generated_trust_authority_owner(&authority)
                        .await?;
                    authority
                        .validate_public_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                        peer.peer_id,
                        &peer.pubkey,
                    )
                    .map_err(SendError::Validation)?;
                    let created = self
                        .add_generated_trust_source(peer, authority.trust_row_owner_kind(), false)
                        .await?;
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                    self.validate_generated_trust_authority_owner(&authority)
                        .await?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_public_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self
                        .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                        .await;
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    self.validate_generated_trust_authority_owner(&authority)
                        .await?;
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    meerkat_core::comms::TrustedPeerDescriptor::validate_pubkey_for_peer_id(
                        peer.peer_id,
                        &peer.pubkey,
                    )
                    .map_err(SendError::Validation)?;
                    let created = self
                        .add_generated_trust_source(peer, authority.trust_row_owner_kind(), true)
                        .await?;
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    self.validate_generated_trust_authority_owner(&authority)
                        .await?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self
                        .remove_generated_trust_source(&peer_id, authority.trust_row_owner_kind())
                        .await;
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
            }
        }

        async fn trusted_peer_projection_snapshot_for_source(
            &self,
            source_kind: GeneratedCommsTrustAuthoritySourceKind,
        ) -> Result<Vec<TrustedPeerDescriptor>, CommsCapabilityError> {
            let descriptors = self.trusted_descriptors.read().await;
            let private_trusted = self.private_trusted.read().await;
            let mut peers = descriptors
                .iter()
                .filter_map(|(peer_id, by_source)| {
                    if private_trusted
                        .get(peer_id)
                        .is_some_and(|sources| sources.contains(&source_kind))
                    {
                        return None;
                    }
                    by_source.get(&source_kind).cloned()
                })
                .collect::<Vec<_>>();
            peers.sort_by(|left, right| left.peer_id.as_str().cmp(&right.peer_id.as_str()));
            Ok(peers)
        }

        async fn install_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self.mob_machine_trust_owner.write().await;
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn validate_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let expected = self.mob_machine_trust_owner.read().await;
            if let Some(existing) = expected.as_ref()
                && !Arc::ptr_eq(existing, &owner)
            {
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            Ok(())
        }

        async fn install_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self.mob_machine_trust_owner.write().await;
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn add_private_trusted_peer(
            &self,
            _peer: meerkat_core::comms::TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            Err(SendError::Unsupported(
                "add_private_trusted_peer requires apply_trust_mutation authority".to_string(),
            ))
        }

        async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            Ok(match cmd {
                CommsCommand::PeerLifecycle { .. } => SendReceipt::PeerLifecycleSent {
                    envelope_id: uuid::Uuid::new_v4(),
                    delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
                },
                _ => SendReceipt::InputAccepted {
                    interaction_id: meerkat_core::interaction::InteractionId(uuid::Uuid::nil()),
                    stream_reserved: false,
                },
            })
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }
    }

    struct MockSessionActor {
        comms: Arc<MockComms>,
        witness: meerkat_session::LiveSessionActorWitness,
    }

    struct MockSessionSvc {
        sessions: RwLock<HashMap<SessionId, MockSessionActor>>,
        /// The spawner restrictions handed to the build of each session
        /// this mock created.
        tool_restrictions: std::sync::Mutex<
            HashMap<SessionId, Result<meerkat_core::PredecessorToolRestrictions, String>>,
        >,
        actor_registry: meerkat_session::LiveSessionActorRegistry,
        persisted_sessions: RwLock<HashMap<SessionId, Session>>,
        archive_failures: RwLock<HashMap<SessionId, String>>,
        keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<Notify>>>,
        last_start_turn: RwLock<Option<(SessionId, String)>>,
        counter: AtomicU64,
        start_turn_delay_ms: AtomicU64,
        start_turn_calls: AtomicU64,
        /// Sessions created or revived through the actor-slot path, the one
        /// every member activation takes.
        session_activations: AtomicU64,
        /// Counting-wrapper guard state: full-document reads through
        /// `load_persisted_session`.
        persisted_full_loads: AtomicU64,
        /// Counting-wrapper guard state: metadata-only reads through
        /// `load_persisted_session_metadata`.
        persisted_metadata_loads: AtomicU64,
        /// Per-session billing usage returned by `read`. Distinct numbers per
        /// session id are what make a session-to-usage mapping assertion
        /// meaningful; unprimed sessions keep reporting zero.
        session_usage: RwLock<HashMap<SessionId, Usage>>,
        runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
        /// The application tool policy of every build, by session, in order.
        build_policies:
            std::sync::Mutex<Vec<(SessionId, meerkat_core::ApplicationToolPolicyBinding)>>,
        /// Scripted owner observations: a session listed here answers
        /// `observe_live_durable_source` with this outcome (an `Err` string is
        /// a store fault) instead of its projection visibility.
        scripted_observations: std::sync::Mutex<
            HashMap<SessionId, Result<meerkat_mob::LiveDurableSourceObservation, String>>,
        >,
        /// Archives of these sessions park until their hold is released.
        archive_holds: std::sync::Mutex<HashMap<SessionId, Arc<InsertHold>>>,
    }

    impl MockSessionSvc {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                tool_restrictions: std::sync::Mutex::new(HashMap::new()),
                actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
                persisted_sessions: RwLock::new(HashMap::new()),
                archive_failures: RwLock::new(HashMap::new()),
                keep_alive_notifiers: RwLock::new(HashMap::new()),
                last_start_turn: RwLock::new(None),
                counter: AtomicU64::new(0),
                start_turn_delay_ms: AtomicU64::new(0),
                start_turn_calls: AtomicU64::new(0),
                session_activations: AtomicU64::new(0),
                persisted_full_loads: AtomicU64::new(0),
                persisted_metadata_loads: AtomicU64::new(0),
                session_usage: RwLock::new(HashMap::new()),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
                build_policies: std::sync::Mutex::default(),
                scripted_observations: std::sync::Mutex::default(),
                archive_holds: std::sync::Mutex::default(),
            }
        }

        /// Park every archive of `id` until the returned hold is released.
        fn hold_archive(&self, id: SessionId) -> Arc<InsertHold> {
            let hold = Arc::new(InsertHold::default());
            self.archive_holds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, Arc::clone(&hold));
            hold
        }

        /// Script what `observe_live_durable_source` reports for `id`.
        fn script_observation(
            &self,
            id: &SessionId,
            outcome: Result<meerkat_mob::LiveDurableSourceObservation, String>,
        ) {
            self.scripted_observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id.clone(), outcome);
        }

        /// Give one session id a distinct billing total.
        async fn prime_session_usage(&self, id: &SessionId, usage: Usage) {
            self.session_usage.write().await.insert(id.clone(), usage);
        }

        /// Make one session unreadable, the way a remote placement or an
        /// archived document is unreadable from this host.
        async fn forget_session(&self, id: &SessionId) {
            self.sessions.write().await.remove(id);
            self.persisted_sessions.write().await.remove(id);
        }

        async fn session_usage_for(&self, id: &SessionId) -> Usage {
            self.session_usage
                .read()
                .await
                .get(id)
                .cloned()
                .unwrap_or_default()
        }

        fn build_policies_snapshot(
            &self,
        ) -> Vec<(SessionId, meerkat_core::ApplicationToolPolicyBinding)> {
            self.build_policies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        /// The application tool policy of every build on `session_id`.
        fn build_policies_for(
            &self,
            session_id: &SessionId,
        ) -> Vec<meerkat_core::ApplicationToolPolicyBinding> {
            self.build_policies_snapshot()
                .into_iter()
                .filter(|(session, _)| session == session_id)
                .map(|(_, policy)| policy)
                .collect()
        }

        async fn cold_restart(&self) -> Self {
            let persisted_sessions = self.persisted_sessions.read().await.clone();
            let archive_failures = self.archive_failures.read().await.clone();
            // The owner's restriction records are durable beside its sessions.
            let tool_restrictions = self
                .tool_restrictions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            Self {
                sessions: RwLock::new(HashMap::new()),
                tool_restrictions: std::sync::Mutex::new(tool_restrictions),
                actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
                persisted_sessions: RwLock::new(persisted_sessions),
                archive_failures: RwLock::new(archive_failures),
                keep_alive_notifiers: RwLock::new(HashMap::new()),
                last_start_turn: RwLock::new(None),
                counter: AtomicU64::new(self.counter.load(Ordering::Relaxed)),
                start_turn_delay_ms: AtomicU64::new(
                    self.start_turn_delay_ms.load(Ordering::Relaxed),
                ),
                start_turn_calls: AtomicU64::new(0),
                session_activations: AtomicU64::new(0),
                persisted_full_loads: AtomicU64::new(0),
                persisted_metadata_loads: AtomicU64::new(0),
                session_usage: RwLock::new(self.session_usage.read().await.clone()),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
                build_policies: std::sync::Mutex::new(self.build_policies_snapshot()),
                scripted_observations: std::sync::Mutex::default(),
                archive_holds: std::sync::Mutex::default(),
            }
        }

        async fn create_session_with_actor_slot(
            &self,
            req: CreateSessionRequest,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            self.session_activations.fetch_add(1, Ordering::Relaxed);
            let build = req.build;
            let tool_restrictions =
                meerkat_core::PredecessorToolRestrictions::from_build_request(build.as_ref())
                    .map_err(|error| error.to_string());
            let mut persisted_session = build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let sid = persisted_session.id().clone();
            if let Some(build) = build.as_ref() {
                self.build_policies
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((sid.clone(), build.application_tool_policy.clone()));
            }
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let is_keep_alive = build
                .as_ref()
                .map(|build| build.keep_alive)
                .unwrap_or(false);
            let provided_bindings =
                build
                    .as_ref()
                    .and_then(|build| match &build.runtime_build_mode {
                        meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
                            Some(bindings.clone())
                        }
                        meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                    });
            let actor_materialization_permit =
                begin_live_actor_materialization(provided_bindings.as_ref())?;
            let name = build
                .as_ref()
                .and_then(|build| build.comms_name.clone())
                .unwrap_or_else(|| format!("s-{n}"));
            if persisted_session.session_metadata().is_none() {
                persisted_session
                    .set_session_metadata(SessionMetadata {
                        model_fallback: None,
                        schema_version: meerkat_core::session_metadata_schema_version(),
                        model: req.model.clone(),
                        max_tokens: req.max_tokens.unwrap_or(4096),
                        structured_output_retries: build
                            .as_ref()
                            .and_then(|options| options.structured_output_retries)
                            .unwrap_or_else(
                                meerkat_core::config::default_structured_output_retries,
                            ),
                        provider: build
                            .as_ref()
                            .and_then(|options| options.provider)
                            .unwrap_or(Provider::Anthropic),
                        self_hosted_server_id: build
                            .as_ref()
                            .and_then(|options| options.self_hosted_server_id.clone()),
                        provider_params: build
                            .as_ref()
                            .and_then(|options| options.provider_params.clone()),
                        tooling: SessionTooling {
                            builtins: build
                                .as_ref()
                                .map(|options| options.override_builtins)
                                .unwrap_or_default(),
                            shell: build
                                .as_ref()
                                .map(|options| options.override_shell)
                                .unwrap_or_default(),
                            comms: build
                                .as_ref()
                                .map(|options| options.override_comms)
                                .unwrap_or_default(),
                            mob: build
                                .as_ref()
                                .map(|options| options.override_mob)
                                .unwrap_or_default(),
                            memory: build
                                .as_ref()
                                .map(|options| options.override_memory)
                                .unwrap_or_default(),
                            schedule: build
                                .as_ref()
                                .map(|options| options.override_schedule)
                                .unwrap_or_default(),
                            workgraph: build
                                .as_ref()
                                .map(|options| options.override_workgraph)
                                .unwrap_or_default(),
                            image_generation: build
                                .as_ref()
                                .map(|options| options.override_image_generation)
                                .unwrap_or_default(),
                            web_search: build
                                .as_ref()
                                .map(|options| options.override_web_search)
                                .unwrap_or_default(),
                            tool_access_policy: build
                                .as_ref()
                                .and_then(|options| options.tool_access_policy.clone()),
                            spawn_tool_access_policy: None,
                            application_tool_policy: build
                                .as_ref()
                                .map(|options| options.application_tool_policy.clone())
                                .unwrap_or_default(),
                            active_skills: build
                                .as_ref()
                                .and_then(|options| options.preload_skills.clone()),
                        },
                        keep_alive: is_keep_alive,
                        comms_name: build
                            .as_ref()
                            .and_then(|options| options.comms_name.clone()),
                        peer_meta: build.as_ref().and_then(|options| options.peer_meta.clone()),
                        realm_id: build.as_ref().and_then(|options| options.realm_id.clone()),
                        instance_id: build
                            .as_ref()
                            .and_then(|options| options.instance_id.clone()),
                        backend: build.as_ref().and_then(|options| {
                            options.backend.map(|kind| kind.as_str().to_string())
                        }),
                        config_generation: build
                            .as_ref()
                            .and_then(|options| options.config_generation),
                        auth_binding: build
                            .as_ref()
                            .and_then(|options| options.auth_binding.clone()),
                        mob_member_binding: build
                            .as_ref()
                            .and_then(|options| options.mob_member_binding.clone()),
                    })
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("failed to seed mock persisted session metadata: {error}"),
                        ))
                    })?;
            }
            let comms = Arc::new(MockComms::new(&name));
            if let Some(bindings) = provided_bindings.as_ref() {
                bindings
                    .install_peer_comms_on(comms.as_ref())
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine peer-comms install failed: {error}"),
                        ))
                    })?;
            }
            let actor_witness = {
                let mut sessions = self.sessions.write().await;
                if sessions.contains_key(&sid) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live session actor is already registered: {sid}"
                        )),
                    ));
                }
                let witness =
                    register_live_actor(&self.actor_registry, actor_witness_slot, sid.clone())?;
                sessions.insert(
                    sid.clone(),
                    MockSessionActor {
                        comms,
                        witness: witness.clone(),
                    },
                );
                witness
            };
            commit_live_actor_materialization_or_discard(
                self,
                actor_materialization_permit,
                &actor_witness,
            )
            .await?;
            self.tool_restrictions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(sid.clone(), tool_restrictions);
            self.persisted_sessions
                .write()
                .await
                .insert(sid.clone(), persisted_session);
            if is_keep_alive {
                self.keep_alive_notifiers
                    .write()
                    .await
                    .insert(sid.clone(), Arc::new(Notify::new()));
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: sid,
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
                run_usage: None,
                request_usage: Vec::new(),
            })
        }

        fn set_turn_delay_ms(&self, delay_ms: u64) {
            self.start_turn_delay_ms.store(delay_ms, Ordering::Relaxed);
        }

        fn session_activation_count(&self) -> u64 {
            self.session_activations.load(Ordering::Relaxed)
        }

        fn start_turn_call_count(&self) -> u64 {
            self.start_turn_calls.load(Ordering::Relaxed)
        }

        async fn last_start_turn(&self) -> Option<(SessionId, String)> {
            self.last_start_turn.read().await.clone()
        }

        async fn insert_persisted_session(&self, session: Session) {
            self.persisted_sessions
                .write()
                .await
                .insert(session.id().clone(), session);
        }

        async fn fail_archive(&self, id: SessionId, reason: impl Into<String>) {
            self.archive_failures
                .write()
                .await
                .insert(id, reason.into());
        }

        async fn clear_archive_failure(&self, id: &SessionId) {
            self.archive_failures.write().await.remove(id);
        }

        async fn session_exists(&self, id: &SessionId) -> bool {
            self.sessions.read().await.contains_key(id)
                || self.persisted_sessions.read().await.contains_key(id)
        }

        async fn retire_with_machine_archive_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            if !self.session_exists(session_id).await
                && !self.runtime_adapter.contains_session(session_id).await
            {
                return Ok(());
            }

            let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
            match meerkat_runtime::RuntimeControlPlane::retire(&*self.runtime_adapter, &runtime_id)
                .await
            {
                Ok(_) => Ok(()),
                Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
                    self.runtime_adapter
                        .register_session(session_id.clone())
                        .await
                        .expect("register session");
                    meerkat_runtime::RuntimeControlPlane::retire(
                        &*self.runtime_adapter,
                        &runtime_id,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("machine archive retire failed after registration: {error}"),
                        ))
                    })
                }
                Err(error) => Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive retire failed: {error}"
                    )),
                )),
            }
        }
    }

    #[async_trait]
    impl SessionService for MockSessionSvc {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
            self.create_session_with_actor_slot(req, &actor_witness_slot)
                .await
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
            *self.last_start_turn.write().await = Some((id.clone(), req.prompt.text_content()));
            let delay_ms = self.start_turn_delay_ms.load(Ordering::Relaxed);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
            // Block only for keep-alive sessions (notifier registered at create time).
            // The notifier is cloned out in its own statement so the map's read
            // guard is released before the turn parks: an `if let` scrutinee
            // guard lives for the whole block, and a keep-alive turn parks until
            // interrupt, so holding it blocked every writer (the next member's
            // create) forever.
            let keep_alive_notifier = self.keep_alive_notifiers.read().await.get(id).cloned();
            if let Some(notifier) = keep_alive_notifier {
                notifier.notified().await;
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
                run_usage: None,
                request_usage: Vec::new(),
            })
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
                notifier.notify_waiters();
            }
            Ok(())
        }

        async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
            Ok(self.sessions.read().await.contains_key(id))
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            let usage = self.session_usage_for(id).await;
            if !self.sessions.read().await.contains_key(id) {
                if self.persisted_sessions.read().await.contains_key(id) {
                    return Ok(SessionView {
                        state: SessionInfo {
                            session_id: id.clone(),
                            created_at: SystemTime::now(),
                            updated_at: SystemTime::now(),
                            message_count: 0,
                            is_active: false,
                            model: "claude-sonnet-4-5".to_string(),
                            provider: Provider::Anthropic,
                            last_assistant_text: None,
                            labels: Default::default(),
                        },
                        billing: SessionUsage {
                            total_tokens: usage.total_tokens(),
                            usage,
                        },
                    });
                }
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionView {
                state: SessionInfo {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    is_active: false,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: Provider::Anthropic,
                    last_assistant_text: None,
                    labels: Default::default(),
                },
                billing: SessionUsage {
                    total_tokens: usage.total_tokens(),
                    usage,
                },
            })
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            Ok(self
                .sessions
                .read()
                .await
                .keys()
                .map(|id| SessionSummary {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    total_tokens: 0,
                    is_active: false,
                    labels: Default::default(),
                })
                .collect())
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            let hold = self
                .archive_holds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id)
                .cloned();
            if let Some(hold) = hold {
                hold.reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hold.release.notified().await;
            }
            if let Some(reason) = self.archive_failures.read().await.get(id).cloned() {
                return Err(SessionError::Unsupported(reason));
            }
            let removed_live = {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(id);
                let removed_registry_actor = self.actor_registry.remove_current(id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {id} during archive"
                        )),
                    ));
                }
                sessions.remove(id).is_some()
            };
            let removed_persisted = self.persisted_sessions.write().await.remove(id).is_some();
            if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
                notifier.notify_waiters();
            }
            if removed_live || removed_persisted {
                // An archived session reports an archive terminal, as the
                // persistent service does.
                self.script_observation(
                    id,
                    Ok(meerkat_mob::LiveDurableSourceObservation::Archived),
                );
                Ok(())
            } else {
                Err(SessionError::NotFound { id: id.clone() })
            }
        }
    }

    #[async_trait]
    impl SessionServiceCommsExt for MockSessionSvc {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|actor| actor.comms.clone() as Arc<dyn CoreCommsRuntime>)
        }

        async fn event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }

        async fn interaction_event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }
    }

    #[async_trait]
    impl SessionServiceControlExt for MockSessionSvc {
        async fn append_system_context(
            &self,
            id: &SessionId,
            _req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() }.into());
            }
            Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Applied,
            })
        }
    }

    #[async_trait]
    impl SessionServiceHistoryExt for MockSessionSvc {
        async fn read_history(
            &self,
            id: &SessionId,
            query: SessionHistoryQuery,
        ) -> Result<SessionHistoryPage, SessionError> {
            if self.sessions.read().await.contains_key(id)
                || self.persisted_sessions.read().await.contains_key(id)
            {
                return Ok(SessionHistoryPage::from_messages(id.clone(), &[], query));
            }
            Err(SessionError::NotFound { id: id.clone() })
        }
    }

    #[async_trait]
    impl MobSessionService for MockSessionSvc {
        async fn predecessor_tool_restrictions(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_core::PredecessorToolRestrictions, SessionError> {
            self.tool_restrictions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session_id)
                .cloned()
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.clone(),
                })?
                .map_err(|reason| {
                    SessionError::Unsupported(format!(
                        "inherited tool restriction evidence for session {session_id}: {reason}"
                    ))
                })
        }

        async fn subscribe_session_activity(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::service::SessionError>
        {
            // This fixture's sessions never report an active turn.
            Ok(meerkat_mob::MemberSessionActivity::inactive())
        }

        async fn observe_member_status_view(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, SessionError> {
            meerkat_mob::observe_member_status_view_via_read(self, session_id).await
        }

        async fn observe_live_durable_source(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, SessionError> {
            let scripted = self
                .scripted_observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session_id)
                .cloned();
            match scripted {
                Some(Ok(observation)) => Ok(observation),
                Some(Err(fault)) => Err(SessionError::Store(fault.into())),
                None => {
                    meerkat_mob::observe_live_durable_source_via_projection_visibility(
                        self, session_id,
                    )
                    .await
                }
            }
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            _source_session_id: &meerkat_core::SessionId,
            _message_count: Option<usize>,
            _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            _target: meerkat_core::DurableSessionForkTarget,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, meerkat_core::service::SessionError>
        {
            Err(meerkat_core::service::SessionError::Unsupported(
                "mock session service has no durable fork authority".to_string(),
            ))
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
            Err(SessionError::Unsupported(
                "mock session service does not support live delegation canonical projection".into(),
            ))
        }

        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
            Err(SessionError::Unsupported(
                "mock session service does not support live delegation canonical projection".into(),
            ))
        }

        async fn materialize_session_resume_verdict(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }

        async fn observe_session_resume_authority(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // This process-local test map has no RuntimeStore authority.
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }

        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            Err(SessionError::Unsupported(
                "mock session service has no store-owned boundary authority".to_string(),
            ))
        }

        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            _session_id: &SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }

        /// Test double: the two-read composition is the exact truth here.
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::runtime::ResumeSessionLoad, meerkat_core::service::SessionError>
        {
            use meerkat_mob::runtime::ResumeSessionLoad;
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            <Self as SessionService>::create_session(self, req).await
        }

        async fn create_session_with_actor_witness_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
            _resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            self.create_session_with_actor_slot(req, actor_witness_slot)
                .await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority(session_id).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
            &self,
            session_id: &SessionId,
            _deadline: meerkat_core::time_compat::Instant,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.discard_live_session(session_id).await
        }

        async fn discard_live_session_actor_under_runtime_turn_boundary(
            &self,
            witness: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<bool, SessionError> {
            let removed = {
                let mut sessions = self.sessions.write().await;
                let Some(actor) = sessions.get(witness.session_id()) else {
                    return Ok(false);
                };
                if !actor.witness.eq(witness) {
                    return Ok(false);
                }
                if !self.actor_registry.compare_remove(witness) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry rejected current witness for {}",
                            witness.session_id()
                        )),
                    ));
                }
                sessions.remove(witness.session_id()).is_some()
            };
            if removed {
                if let Some(notifier) = self
                    .keep_alive_notifiers
                    .write()
                    .await
                    .remove(witness.session_id())
                {
                    notifier.notify_waiters();
                }
            }
            Ok(removed)
        }

        async fn live_session_actor_registered(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            Ok(self.actor_registry.contains(session_id))
        }

        fn supports_runtime_turn_apply(&self) -> bool {
            true
        }

        fn acquire_runtime_adapter(
            &self,
            explicit: Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>>,
        ) -> Result<
            Option<std::sync::Arc<meerkat_runtime::MeerkatMachine>>,
            meerkat_runtime::RuntimeDriverError,
        > {
            let owner = Some(self.runtime_adapter.clone());
            if let (Some(owner), Some(requested)) = (owner.as_ref(), explicit.as_ref())
                && !owner.shares_runtime_execution_owner_with(requested)
            {
                return Err(
                    meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        reason:
                            meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                    },
                );
            }
            Ok(owner.or(explicit))
        }

        async fn archive_with_mob_lifecycle_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.retire_with_machine_archive_authority(session_id)
                .await?;
            SessionService::archive(self, session_id).await
        }

        async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
            {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(session_id);
                let removed_registry_actor = self.actor_registry.remove_current(session_id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {session_id} during discard"
                        )),
                    ));
                }
                sessions.remove(session_id);
            }
            if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(session_id) {
                notifier.notify_waiters();
            }
            Ok(())
        }

        /// A session belongs to the mob its persisted typed member binding
        /// names, as a real store reports it. Several mobs can share this
        /// service, and resume reconciliation archives live sessions it is
        /// told belong to the resuming mob: claiming every session for every
        /// mob made one mob's restore archive another mob's member. A session
        /// without a persisted binding keeps the earlier answer.
        async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
            let Some(session) = self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned()
            else {
                return true;
            };
            match meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
                .ok()
                .as_ref()
                .and_then(super::persisted_mob_binding)
            {
                Some(owner) => &owner == mob_id,
                None => true,
            }
        }

        async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<Session>, SessionError> {
            self.persisted_full_loads.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned())
        }

        async fn load_persisted_session_metadata(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.persisted_metadata_loads
                .fetch_add(1, Ordering::Relaxed);
            let Some(session) = self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned()
            else {
                return Ok(None);
            };
            meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
                .map(Some)
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "session {session_id} durable metadata failed typed restore: {err}"
                    )))
                })
        }

        async fn apply_runtime_turn(
            &self,
            session_id: &SessionId,
            run_id: meerkat_core::RunId,
            req: StartTurnRequest,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<meerkat_core::InputId>,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
            let run_result = <Self as SessionService>::start_turn(self, session_id, req).await?;
            Ok(
                meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                    meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                        run_id,
                        boundary,
                        contributing_input_ids,
                        conversation_digest: None,
                        message_count: 0,
                    },
                    None,
                    run_result,
                ),
            )
        }
    }

    #[tokio::test]
    async fn local_session_service_persists_appended_context() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let result = service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                },
            )
            .await
            .expect("append context");
        assert_eq!(result.status, AppendSystemContextStatus::Applied);
        let pending = service.system_messages.read().await;
        assert_eq!(pending.get(&session_id).map(std::vec::Vec::len), Some(1));
    }

    /// Readiness agrees with the open (#1246): an in-memory session has no
    /// persisted durable source, so the live open's body load finds nothing
    /// and readiness must not report it as committed.
    #[tokio::test]
    async fn local_session_service_live_readiness_agrees_with_its_open() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        assert!(
            meerkat_mob::MobSessionService::load_persisted_session(&service, &session_id)
                .await
                .expect("load persisted session")
                .is_none(),
            "control: the open's durable-source load finds no persisted body"
        );
        assert_eq!(
            meerkat_mob::MobSessionService::observe_live_durable_source(&service, &session_id)
                .await
                .expect("observe live durable source"),
            meerkat_mob::LiveDurableSourceObservation::Absent,
            "a live in-memory session is not a committed durable source"
        );
        assert_eq!(
            meerkat_mob::MobSessionService::observe_live_durable_source(
                &service,
                &SessionId::new()
            )
            .await
            .expect("observe unknown session"),
            meerkat_mob::LiveDurableSourceObservation::Absent
        );
    }

    #[tokio::test]
    async fn local_session_service_does_not_fold_system_messages_into_user_prompt()
    -> Result<(), String> {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                },
            )
            .await
            .expect("append context");

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        HandlingMode::Queue,
                        None,
                        None,
                    ),
                },
            )
            .await
            .expect("start turn");

        let first = stream.next().await.expect("first event");
        match first.payload {
            AgentEvent::RunStarted { input, .. } => {
                let prompt = input
                    .content()
                    .expect("content-bearing run input")
                    .text_content();
                assert_eq!(prompt, "hello");
            }
            other => return Err(format!("expected RunStarted, got {other:?}")),
        }

        let system_messages = service.system_messages.read().await;
        assert_eq!(
            system_messages.get(&session_id).map(std::vec::Vec::len),
            Some(1)
        );
        Ok(())
    }

    #[tokio::test]
    async fn local_session_service_preserves_multimodal_prompt_with_system_messages()
    -> Result<(), String> {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the picture.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-image".to_string()),
                },
            )
            .await
            .expect("append context");

        let prompt = meerkat_core::types::ContentInput::Blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: "Look at this.".to_string(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc123".into(),
            },
        ]);

        match prompt {
            meerkat_core::types::ContentInput::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(
                    blocks.first(),
                    Some(meerkat_core::types::ContentBlock::Text { text }) if text == "Look at this."
                ));
                assert!(matches!(
                    blocks.get(1),
                    Some(meerkat_core::types::ContentBlock::Image { media_type, .. }) if media_type == "image/png"
                ));
            }
            other => return Err(format!("expected multimodal blocks, got {other:?}")),
        }
        Ok(())
    }

    #[tokio::test]
    async fn local_session_service_archive_retains_system_messages() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Remember the customer preference.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-archive".to_string()),
                },
            )
            .await
            .expect("append context");

        service.archive(&session_id).await.expect("archive session");

        let messages = service.system_messages.read().await;
        assert_eq!(
            messages.get(&session_id).map(std::vec::Vec::len),
            Some(1),
            "archive must retain ordinary System transcript entries"
        );
    }

    #[tokio::test]
    async fn local_session_service_publishes_boundary_discard_with_exact_actor_and_sequence() {
        use futures::{FutureExt, StreamExt};
        use meerkat_core::event::BoundaryAppendsDiscarded;

        let service = LocalSessionService::new();
        let request = || CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        let created = service
            .create_session_with_actor_slot(request(), &slot)
            .await
            .unwrap();
        let witness = slot.witness().unwrap();
        let session_id = created.session_id;
        let mut events =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .unwrap();
        let discarded = BoundaryAppendsDiscarded {
            session_id: session_id.clone(),
            run_id: meerkat_core::lifecycle::RunId::new(),
            input_ids: vec![meerkat_core::lifecycle::InputId::new()],
        };
        let mut last_seq = 0;
        for _ in 0..2 {
            service
                .start_turn(
                    &session_id,
                    StartTurnRequest {
                        injected_context: Vec::new(),
                        prompt: "continue".to_string().into(),
                        system_prompt: None,
                        event_tx: None,
                        runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                            HandlingMode::Queue,
                            None,
                            None,
                        ),
                    },
                )
                .await
                .unwrap();
            for _ in 0..4 {
                let event = tokio::time::timeout(Duration::from_secs(2), events.next())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(event.seq, last_seq + 1);
                last_seq = event.seq;
            }
            service
                .publish_boundary_appends_discarded_for_actor(&witness, &discarded)
                .await
                .expect("local session owner must publish an exact discard");
            let event = tokio::time::timeout(Duration::from_secs(2), events.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.seq, last_seq + 1);
            last_seq = event.seq;
            assert_eq!(event.source_session_id(), Some(&session_id));
            assert!(matches!(event.payload,
                AgentEvent::BoundaryAppendsDiscarded(value) if value == discarded));
        }
        let mut mismatch = discarded.clone();
        mismatch.session_id = SessionId::new();
        assert!(matches!(
            service
                .publish_boundary_appends_discarded_for_actor(&witness, &mismatch)
                .await,
            Err(SessionError::Agent(_))
        ));
        let mut empty = discarded.clone();
        empty.input_ids.clear();
        service
            .publish_boundary_appends_discarded_for_actor(&witness, &empty)
            .await
            .unwrap();
        assert!(events.next().now_or_never().is_none());

        assert!(
            service
                .discard_live_session_actor_under_runtime_turn_boundary(&witness)
                .await
                .unwrap()
        );
        let mut replacement = request();
        replacement.build = Some(meerkat_core::service::SessionBuildOptions {
            resume_session: Some(Session::with_id(session_id.clone())),
            ..Default::default()
        });
        let replacement_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        service
            .create_session_with_actor_slot(replacement, &replacement_slot)
            .await
            .unwrap();
        let current = replacement_slot.witness().unwrap();
        let mut current_events =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .unwrap();
        assert!(matches!(
            service.publish_boundary_appends_discarded_for_actor(&witness, &discarded).await,
            Err(SessionError::NotFound { id }) if id == session_id
        ));
        assert!(current_events.next().now_or_never().is_none());
        service
            .publish_boundary_appends_discarded_for_actor(&current, &discarded)
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), current_events.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event.payload,
            AgentEvent::BoundaryAppendsDiscarded(value) if value == discarded));
    }

    #[tokio::test]
    async fn local_session_service_stream_emits_events_on_turn() {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        let turn = service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: "hello".to_string().into(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        HandlingMode::Queue,
                        None,
                        None,
                    ),
                },
            )
            .await;
        assert!(turn.is_ok());

        let first = stream.next().await.expect("first event");
        assert!(matches!(first.payload, AgentEvent::RunStarted { .. }));
        let second = stream.next().await.expect("second event");
        assert!(matches!(second.payload, AgentEvent::TurnStarted { .. }));
    }

    fn mk_call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "t1",
            name,
            args,
        }
    }

    async fn call_tool(
        d: &MobMcpDispatcher,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        let out = d.dispatch(mk_call(name, &raw)).await.expect("tool call");
        serde_json::from_str(&out.result.text_content()).expect("tool json")
    }

    async fn call_tool_err(d: &MobMcpDispatcher, name: &str, args: serde_json::Value) -> ToolError {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        d.dispatch(mk_call(name, &raw))
            .await
            .expect_err("tool call should fail")
    }

    #[test]
    fn test_map_destroy_err_preserves_incomplete_error_data() {
        let raw = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        let call = mk_call("mob_lifecycle", &raw);
        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker: archive failed".to_string());

        let error = map_destroy_err(call, MobMcpDestroyError::Incomplete { report });
        let data = error
            .structured_data()
            .expect("incomplete destroy should include structured data");

        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(data.get("destroy_report").is_some());
    }

    #[tokio::test]
    async fn test_dispatcher_exposes_expected_tools() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);
        let tools = d.tools();
        let tool_names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(
            tool_names,
            vec![
                "mob_create",
                "mob_list",
                "mob_lifecycle",
                "mob_events",
                "mob_run_flow",
                "mob_flow_status",
                "mob_cancel_flow",
                "mob_spawn_member",
                "mob_retire_member",
                "mob_list_members",
                "mob_wire",
                "mob_respawn",
                "mob_force_cancel",
                "mob_member_status",
                "mob_wait_kickoff",
                "mob_wait_ready",
            ]
        );
    }

    #[tokio::test]
    async fn test_mob_lifecycle_rejects_unknown_action_at_contract_boundary() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"typed_agent_lifecycle_rejects_unknown","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;

        let error = call_tool_err(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown", "action": "explode"}),
        )
        .await;
        let ToolError::InvalidArguments { reason, .. } = error else {
            panic!("unknown lifecycle action must be InvalidArguments, got: {error:?}");
        };
        assert!(
            reason.contains("unknown variant") && !reason.contains("unknown lifecycle action"),
            "unexpected error: {reason}"
        );

        let status = call_tool(
            &d,
            "mob_list",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown"}),
        )
        .await;
        assert_eq!(status["status"], "Running");

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_rejects_unknown", "action": "destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mob_lifecycle_accepts_typed_contract_params() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"typed_agent_lifecycle_complete","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;

        let params = MobLifecycleParams {
            mob_id: "typed_agent_lifecycle_complete".to_string(),
            action: WireMobLifecycleAction::Complete,
        };
        let payload = serde_json::to_value(&params).expect("typed lifecycle params serialize");
        let result = call_tool(&d, "mob_lifecycle", payload).await;

        assert_eq!(result["ok"], true);

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": "typed_agent_lifecycle_complete", "action": "destroy"}),
        )
        .await;
    }

    #[test]
    fn test_mob_wire_schema_uses_external_binding_without_raw_peer_atoms() {
        let tools = tools_list();
        let schema = tools
            .iter()
            .find(|tool| tool["name"] == "mob_wire")
            .and_then(|tool| tool.get("inputSchema"))
            .expect("mob_wire schema present");
        let schema_text = serde_json::to_string(schema).expect("schema should encode");

        assert!(schema_text.contains("external_binding"));
        assert!(
            !schema_text.contains("\"peer_id\"") && !schema_text.contains("\"pubkey\""),
            "mob_wire schema must not expose raw comms identity atoms: {schema_text}"
        );
    }

    #[tokio::test]
    async fn test_owns_persisted_session_requires_actual_roster_membership() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(
            MobMcpState::new(session_service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let dispatcher = MobMcpDispatcher::new(Arc::clone(&state));

        call_tool(
            &dispatcher,
            "mob_create",
            json!({
                "definition": {
                    "id": "team",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-8",
                            "tools": {"comms": true, "mob": true},
                            "external_addressable": true
                        }
                    },
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {"default": "session"}
                }
            }),
        )
        .await;

        let mut spoofed = Session::new();
        let spoofed_id = spoofed.id().clone();
        let _ = spoofed.set_session_metadata(SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            // Spoof: a session that merely carries the comms_name shape but no
            // typed durable binding must NOT be owned. Identity is the typed
            // mob_member_binding, not the routing-name string.
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        });
        svc.insert_persisted_session(spoofed).await;

        assert!(
            !state.owns_persisted_bridge_session(&spoofed_id).await,
            "persisted session routing must verify real mob membership via the typed binding, not the comms_name shape"
        );
    }

    #[tokio::test]
    async fn test_owns_persisted_bridge_session_accepts_mob_marked_session_without_live_handle() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(
            MobMcpState::new(session_service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            // Transport routing name + discovery metadata (kept as-is); the
            // realm now uses the canonical dot form via the shared helper.
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: Some(
                PeerMeta::default()
                    .with_label("mob_id", "team")
                    .with_label("role", "reviewer")
                    .with_label("meerkat_id", "alice"),
            ),
            realm_id: Some(meerkat_core::RealmId::parse("mob.team").unwrap()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            // Typed durable identity — this is what ownership routing reads.
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        assert!(
            state.owns_persisted_bridge_session(&persisted_id).await,
            "persisted mob members must still route through mob ownership after restart even before a live handle is rehydrated"
        );
    }

    /// Counting-wrapper guard (meta ask-24 clause 3): the persisted
    /// ownership probe reads the metadata seam ONLY — exactly one
    /// metadata-view load and ZERO full session-document loads. Regresses if
    /// `owns_persisted_bridge_session` (or anything on its path) reaches for
    /// `load_persisted_session` again.
    #[tokio::test]
    async fn test_owns_persisted_bridge_session_reads_metadata_seam_without_full_load() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(
            MobMcpState::new(session_service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        assert!(
            state.owns_persisted_bridge_session(&persisted_id).await,
            "the metadata seam must resolve ownership for a bound persisted session"
        );
        assert_eq!(
            svc.persisted_full_loads.load(Ordering::Relaxed),
            0,
            "the ownership probe must not materialize the full session document"
        );
        assert_eq!(
            svc.persisted_metadata_loads.load(Ordering::Relaxed),
            1,
            "the ownership probe reads exactly one metadata view"
        );
    }

    #[tokio::test]
    async fn test_retire_member_by_bridge_session_id_falls_back_to_archiving_persisted_member() {
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(
            MobMcpState::new(session_service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let mut persisted = Session::new();
        let persisted_id = persisted.id().clone();
        let _ = persisted.set_session_metadata(SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                comms: ToolCategoryOverride::Enable,
                ..SessionTooling::default()
            },
            keep_alive: false,
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: Some(
                PeerMeta::default()
                    .with_label("mob_id", "team")
                    .with_label("role", "reviewer")
                    .with_label("meerkat_id", "alice"),
            ),
            realm_id: Some(meerkat_core::RealmId::parse("mob.team").unwrap()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            // Ownership routing reads the typed binding directly.
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        });
        svc.insert_persisted_session(persisted).await;

        state
            .retire_member_by_bridge_session_id(&persisted_id)
            .await
            .expect("persisted mob sessions should archive cleanly even without a live handle");
        assert!(
            svc.load_persisted_session(&persisted_id)
                .await
                .expect("load persisted")
                .is_none(),
            "archive fallback must remove the persisted session snapshot"
        );
    }

    #[tokio::test]
    async fn test_retire_member_unknown_bridge_session_returns_typed_recovery_variant() {
        // A bridge session that lives in no live roster, is not service-reported,
        // and is not persisted must surface the TYPED recovery class
        // (BridgeSessionNotInLiveAuthority) rather than an Internal string the
        // consumer has to re-parse by message prefix.
        let svc = Arc::new(MockSessionSvc::new());
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = svc.clone();
        let state = Arc::new(
            MobMcpState::new(session_service, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let unknown = SessionId::new();
        let err = state
            .retire_member_by_bridge_session_id(&unknown)
            .await
            .expect_err("unknown bridge session must not retire successfully");
        match err {
            MobError::BridgeSessionNotInLiveAuthority { bridge_session_id } => {
                assert_eq!(bridge_session_id, unknown.to_string());
            }
            other => panic!("expected typed BridgeSessionNotInLiveAuthority, got {other:?}"),
        }
    }

    // `mob_id` must be unique per test: the supervisor binds
    // `<mob_id>/__mob_supervisor__` in the process-global inproc registry, and
    // parallel tests creating the same mob fail with a live-route rejection
    // (2026-09-23 native unit lane, nine tests).
    fn flow_enabled_definition(mob_id: &str) -> serde_json::Value {
        json!({
            "id": mob_id,
            "orchestrator": {
                "profile": "lead"
            },
            "profiles": {
                "lead": {
                    "model": "claude-opus-4-8",
                    "external_addressable": true,
                    "peer_description": "Lead",
                    "tools": {
                        "comms": true,
                        "mob": true
                    }
                },
                "worker": {
                    "model": "claude-sonnet-4-5",
                    "external_addressable": false,
                    "peer_description": "Worker",
                    "tools": {
                        "comms": true,
                        "mob": true
                    }
                }
            },
            "wiring": {
                "auto_wire_orchestrator": false,
                "role_wiring": []
            },
            "backend": {
                "default": "session"
            },
            "flows": {
                "demo": {
                    "description": "demo flow",
                    "steps": {
                        "start": {
                            "role": "worker",
                            "message": "run demo",
                            // Explicit: the cancel test depends on the mock's
                            // non-JSON "ok" turn output remaining a parse
                            // fault; the schema-aware omitted default would
                            // resolve to text and complete the step.
                            "output_format": "json",
                            "timeout_ms": 1000
                        }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn test_multi_mob_isolation() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let a = call_tool(&d, "mob_create", json!({"definition":{"id":"mob_a","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        let b = call_tool(&d, "mob_create", json!({"definition":{"id":"mob_b","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": a, "specs":[{"profile":"worker", "agent_identity":"wa", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": b, "specs":[{"profile":"worker", "agent_identity":"wb", "runtime_mode":"turn_driven"}]}),
        )
        .await;

        let la = call_tool(&d, "mob_list_members", json!({"mob_id": a})).await;
        let lb = call_tool(&d, "mob_list_members", json!({"mob_id": b})).await;
        assert_eq!(la["members"].as_array().unwrap().len(), 1); // wa
        assert_eq!(lb["members"].as_array().unwrap().len(), 1); // wb

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": a, "action":"destroy"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": b, "action":"destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mcp_e2e_flow_and_destroy_removes_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-e2e-flow-and-destroy-removes-mob","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "agent_identity":"lead", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w2", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"wire"}),
        )
        .await;
        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        assert_eq!(
            listed["members"].as_array().map(std::vec::Vec::len),
            Some(3)
        );
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"unwire"}),
        )
        .await;
        call_tool(
            &d,
            "mob_retire_member",
            json!({"mob_id": mob_id, "agent_identity":"w2"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"complete"}),
        )
        .await;
        let events = call_tool(
            &d,
            "mob_events",
            json!({"mob_id": mob_id, "after_cursor":0, "limit":50}),
        )
        .await;
        let events = events["events"].as_array().cloned().unwrap_or_default();
        assert!(
            events.iter().any(|e| e["kind"]["type"] == "member_spawned"),
            "expected structural events to include member_spawned"
        );
        assert!(
            events.iter().any(|e| e["kind"]["type"] == "mob_completed"),
            "expected structural events to include mob_completed"
        );
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"destroy"}),
        )
        .await;
        let listed = call_tool(&d, "mob_list", json!({})).await;
        assert!(listed["mobs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mob_list_observes_without_waiting_for_in_flight_member_turn() {
        let svc = Arc::new(MockSessionSvc::new());
        svc.set_turn_delay_ms(5_000);
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(Arc::clone(&state));

        let mob_id = state
            .mob_create_definition(explicit_definition("self-observation-mob"))
            .await
            .expect("create mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");

        let baseline_start_turn_calls = svc.start_turn_call_count();
        let turn_handle = state.handle_for(&mob_id).await.expect("handle");
        let in_flight_turn = tokio::spawn(async move {
            turn_handle
                .member(&AgentIdentity::from("worker-1"))
                .await?
                .internal_turn("observe the mob")
                .await
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while svc.start_turn_call_count() <= baseline_start_turn_calls {
            assert!(
                Instant::now() < deadline,
                "member turn should reach the delayed session service"
            );
            sleep(Duration::from_millis(10)).await;
        }

        let listed = tokio::time::timeout(
            Duration::from_millis(100),
            call_tool(&d, "mob_list", json!({})),
        )
        .await
        .expect("mob_list must not wait behind the in-flight member turn");
        assert_eq!(listed["mobs"].as_array().unwrap().len(), 1);
        assert_eq!(listed["mobs"][0]["status"], "Running");

        let profiles = tokio::time::timeout(Duration::from_millis(100), state.realm_profile_list())
            .await
            .expect("profile observation should also remain available during the turn")
            .expect("profile list should succeed");
        drop(profiles);

        in_flight_turn.abort();
    }

    #[tokio::test]
    async fn test_mcp_stop_resume_round_trip() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-stop-resume-round-trip","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "agent_identity":"lead", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w2", "runtime_mode":"turn_driven"}]}),
        )
        .await;
        call_tool(
            &d,
            "mob_wire",
            json!({"mob_id": mob_id, "agent_identity":"w1", "peer":{"local":"w2"}, "action":"wire"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"stop"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"resume"}),
        )
        .await;
        let members = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        assert_eq!(members["members"].as_array().unwrap().len(), 3); // lead + 2 workers
        let status = call_tool(&d, "mob_list", json!({"mob_id": mob_id})).await;
        assert_eq!(status["status"], "Running");

        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"destroy"}),
        )
        .await;
    }

    #[tokio::test]
    async fn test_mcp_flow_tools_dispatch_run_status_cancel() {
        let svc = Arc::new(MockSessionSvc::new());
        svc.set_turn_delay_ms(60_000);
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": flow_enabled_definition("flow-mob-flow-tools-dispatch-run-status-cancel")
            }),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_id should be returned")
            .to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "agent_identity":"w1"}]}),
        )
        .await;

        let started = call_tool(
            &d,
            "mob_run_flow",
            json!({
                "mob_id": mob_id,
                "flow_id": "demo",
                "params": {"ticket": "ABC-123"}
            }),
        )
        .await;
        let run_id = started["run_id"]
            .as_str()
            .expect("run_id should be returned")
            .to_string();

        let status = call_tool(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(status["run"]["run_id"], run_id);
        assert_eq!(status["run"]["flow_id"], "demo");

        let canceled = call_tool(
            &d,
            "mob_cancel_flow",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(canceled["ok"], true);

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut terminal_status = None;
        while Instant::now() < deadline {
            let polled = call_tool(
                &d,
                "mob_flow_status",
                json!({
                    "mob_id": mob_id.clone(),
                    "run_id": run_id.clone()
                }),
            )
            .await;
            if let Some(status) = polled
                .get("run")
                .and_then(|run| run.get("status"))
                .and_then(|status| status.as_str())
            {
                if matches!(status, "canceled" | "completed" | "failed") {
                    terminal_status = Some(status.to_string());
                    break;
                }
            }
            sleep(Duration::from_millis(25)).await;
        }
        assert!(
            matches!(terminal_status.as_deref(), Some("canceled" | "failed")),
            "mob_cancel_flow should converge to canceled, or failed if terminal failure won the race first; got {terminal_status:?}"
        );
    }

    #[tokio::test]
    async fn workgraph_flow_bridge_projects_terminal_failure_before_retry_eligibility() {
        let svc = Arc::new(MockSessionSvc::new());
        let workgraph =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_workgraph_service(Some(workgraph.clone())),
        );
        let dispatcher = MobMcpDispatcher::new(state.clone());

        let created = call_tool(
            &dispatcher,
            "mob_create",
            json!({ "definition": flow_enabled_definition("flow-mob-workgraph-flow-bridge-projects-terminal-failure-before-retry-eligibility") }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().expect("mob id").to_string();
        call_tool(
            &dispatcher,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "w1"}]
            }),
        )
        .await;

        let item = workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Prove terminal Flow evidence projection".to_string(),
                ..Default::default()
            })
            .await
            .expect("create work item");
        let launched = state
            .launch_workgraph_flow(LaunchWorkGraphFlowRequest {
                work_item_id: item.id.clone(),
                realm_id: Some(item.realm_id.clone()),
                namespace: Some(item.namespace.clone()),
                expected_item_revision: item.revision,
                mob_id: meerkat_mob::MobId::from(mob_id.as_str()),
                flow_id: meerkat_mob::FlowId::from("demo"),
                activation_params: json!({"ticket": "WG-1"}),
                idempotency_key: "workgraph-flow-failure-evidence".to_string(),
                supersedes: None,
            })
            .await
            .expect("launch bound Flow");

        let binding_id = launched.binding.binding_id;
        let deadline = Instant::now() + Duration::from_secs(8);
        let terminal = loop {
            let reconciled = state
                .reconcile_workgraph_flow(
                    Some(item.realm_id.clone()),
                    Some(item.namespace.clone()),
                    binding_id.clone(),
                )
                .await
                .expect("reconcile bound Flow");
            if reconciled
                .run
                .as_ref()
                .is_some_and(|run| matches!(run.status, meerkat_mob::MobRunStatus::Failed))
                && reconciled.evidence_projected
            {
                break reconciled;
            }
            assert!(Instant::now() < deadline, "bound Flow did not terminalize");
            sleep(Duration::from_millis(25)).await;
        };

        assert_eq!(terminal.item.status, meerkat::WorkStatus::Open);
        assert!(terminal.item.evidence_refs.iter().any(|evidence| {
            evidence.id == terminal.binding.evidence_id() && evidence.kind == "mob_flow_run_failed"
        }));
        assert!(matches!(
            meerkat::WorkExecutionMachine::recover_effect(&terminal.binding)
                .expect("recover terminal binding"),
            meerkat::WorkExecutionLifecycleEffect::FlowFailed { .. }
        ));
    }

    #[tokio::test]
    async fn workgraph_flow_bridge_closes_only_after_evidence_and_replays_exact_launch() {
        let svc = Arc::new(MockSessionSvc::new());
        let workgraph =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_workgraph_service(Some(workgraph.clone())),
        );
        let dispatcher = MobMcpDispatcher::new(state.clone());
        let mut definition = flow_enabled_definition(
            "flow-mob-workgraph-flow-bridge-closes-only-after-evidence-and-replays-exact-launch",
        );
        definition["flows"]["demo"]["steps"]["start"]["output_format"] = json!("text");

        let created = call_tool(
            &dispatcher,
            "mob_create",
            json!({ "definition": definition }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().expect("mob id").to_string();
        call_tool(
            &dispatcher,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "w1"}]
            }),
        )
        .await;

        let item = workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Prove successful Flow evidence adjudication".to_string(),
                ..Default::default()
            })
            .await
            .expect("create work item");
        let request = LaunchWorkGraphFlowRequest {
            work_item_id: item.id.clone(),
            realm_id: Some(item.realm_id.clone()),
            namespace: Some(item.namespace.clone()),
            expected_item_revision: item.revision,
            mob_id: meerkat_mob::MobId::from(mob_id.as_str()),
            flow_id: meerkat_mob::FlowId::from("demo"),
            activation_params: json!({"ticket": "WG-2"}),
            idempotency_key: "workgraph-flow-success-evidence".to_string(),
            supersedes: None,
        };
        let launched = state
            .launch_workgraph_flow(request.clone())
            .await
            .expect("launch bound Flow");

        let deadline = Instant::now() + Duration::from_secs(8);
        let completed = loop {
            let reconciled = state
                .reconcile_workgraph_flow(
                    Some(item.realm_id.clone()),
                    Some(item.namespace.clone()),
                    launched.binding.binding_id.clone(),
                )
                .await
                .expect("reconcile bound Flow");
            if reconciled.work_item_closed {
                break reconciled;
            }
            assert!(Instant::now() < deadline, "bound Flow did not close work");
            sleep(Duration::from_millis(25)).await;
        };

        assert!(completed.evidence_projected);
        assert_eq!(completed.item.status, meerkat::WorkStatus::Completed);
        assert!(matches!(
            meerkat::WorkExecutionMachine::recover_effect(&completed.binding)
                .expect("recover closed binding"),
            meerkat::WorkExecutionLifecycleEffect::WorkClosed { .. }
        ));

        let replayed = state
            .launch_workgraph_flow(request)
            .await
            .expect("replay exact launch after work closure");
        assert_eq!(replayed.binding.binding_id, completed.binding.binding_id);
        assert_eq!(
            replayed.run.as_ref().map(|run| &run.run_id),
            completed.run.as_ref().map(|run| &run.run_id)
        );
        assert!(replayed.work_item_closed);
    }

    #[tokio::test]
    async fn public_mcp_dispatches_durable_workgraph_flow_launch_and_reconcile() {
        let svc = Arc::new(MockSessionSvc::new());
        let workgraph =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_workgraph_service(Some(workgraph.clone())),
        );
        let dispatcher = MobMcpDispatcher::new(state.clone());
        let created = call_tool(
            &dispatcher,
            "mob_create",
            json!({ "definition": flow_enabled_definition("flow-mob-public-mcp-dispatches-durable-workgraph-flow-launch-and-reconcile") }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().expect("mob id").to_string();
        call_tool(
            &dispatcher,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "w1"}]
            }),
        )
        .await;
        let item = workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Public bridge contract".to_string(),
                ..Default::default()
            })
            .await
            .expect("work item");

        let launched = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_workgraph_flow_launch",
            &json!({
                "work_item_id": item.id,
                "realm_id": item.realm_id,
                "namespace": item.namespace,
                "expected_item_revision": item.revision,
                "mob_id": mob_id,
                "flow_id": "demo",
                "params": {"ticket": "PUBLIC-1"},
                "idempotency_key": "public-workgraph-flow"
            }),
        )
        .await
        .expect("public launch");
        let binding_id = launched["binding"]["binding_id"]
            .as_str()
            .expect("binding id")
            .to_string();
        assert!(launched["run"]["run_id"].is_string());
        assert!(launched["binding"].get("activation_params").is_none());
        assert!(launched["binding"].get("idempotency_key").is_none());
        assert!(launched["binding"].get("correlation_id").is_none());
        assert!(launched["binding"].get("execution_authority").is_none());

        let binding = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_workgraph_flow_binding_get",
            &json!({
                "binding_id": binding_id,
                "realm_id": workgraph.default_realm_id(),
                "namespace": "default"
            }),
        )
        .await
        .expect("public binding get");
        assert_eq!(
            binding["binding"]["binding_id"],
            launched["binding"]["binding_id"]
        );
        assert!(binding["binding"].get("activation_params").is_none());

        let bindings = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_workgraph_flow_binding_list",
            &json!({
                "realm_id": workgraph.default_realm_id(),
                "namespace": "default",
                "limit": 10
            }),
        )
        .await
        .expect("public binding list");
        assert_eq!(
            bindings["bindings"][0]["binding_id"],
            binding["binding"]["binding_id"]
        );

        let status = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_flow_status",
            &json!({
                "mob_id": mob_id,
                "run_id": launched["run"]["run_id"]
            }),
        )
        .await
        .expect("public flow status with reverse binding");
        assert_eq!(
            status["execution_binding"]["binding_id"],
            launched["binding"]["binding_id"]
        );

        let reconciled = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_workgraph_flow_reconcile",
            &json!({
                "binding_id": binding_id,
                "realm_id": workgraph.default_realm_id(),
                "namespace": "default"
            }),
        )
        .await
        .expect("public reconcile");
        assert_eq!(
            reconciled["binding"]["binding_id"],
            launched["binding"]["binding_id"]
        );
    }

    #[tokio::test]
    async fn workgraph_flow_bridge_observes_exact_run_without_broad_list_grant() {
        let svc = Arc::new(MockSessionSvc::new());
        let owner = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let dispatcher = MobMcpDispatcher::new(owner.clone());
        let created = call_tool(
            &dispatcher,
            "mob_create",
            json!({ "definition": flow_enabled_definition("flow-mob-workgraph-flow-bridge-observes-exact-run-without-broad-list-grant") }),
        )
        .await;
        let mob_id = meerkat_mob::MobId::from(created["mob_id"].as_str().expect("mob id"));
        call_tool(
            &dispatcher,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "w1"}]
            }),
        )
        .await;
        let principal =
            meerkat_core::auth::PrincipalId::new("workgraph-flow-operator").expect("principal");
        owner
            .mob_grant_scopes(
                &mob_id,
                principal.clone(),
                BTreeSet::from([ControlScope::SendCommand]),
                None,
            )
            .await
            .expect("grant only SendCommand");

        let workgraph =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        let operator = Arc::new(
            MobMcpState::new(svc, MobControlPrincipal::External(principal))
                .expect("construct runtime authority")
                .with_workgraph_service(Some(workgraph.clone())),
        );
        operator
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;
        let item = workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Scoped bridge observation".to_string(),
                ..Default::default()
            })
            .await
            .expect("item");
        let launched = operator
            .launch_workgraph_flow(LaunchWorkGraphFlowRequest {
                work_item_id: item.id,
                realm_id: Some(item.realm_id),
                namespace: Some(item.namespace),
                expected_item_revision: item.revision,
                mob_id,
                flow_id: FlowId::from("demo"),
                activation_params: json!({"scope": "send-only"}),
                idempotency_key: "send-only-bridge".to_string(),
                supersedes: None,
            })
            .await
            .expect("SendCommand-only principal launches and observes exact bound run");
        assert!(launched.run.is_some());
    }

    #[tokio::test]
    async fn revoked_launch_authority_terminalizes_precommitted_binding() {
        let svc = Arc::new(MockSessionSvc::new());
        let owner = Arc::new(
            MobMcpState::new(svc.clone(), MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let dispatcher = MobMcpDispatcher::new(owner.clone());
        let created = call_tool(
            &dispatcher,
            "mob_create",
            json!({ "definition": flow_enabled_definition("flow-mob-revoked-launch-authority-terminalizes-precommitted-binding") }),
        )
        .await;
        let mob_id = MobId::from(created["mob_id"].as_str().expect("mob id"));
        let principal =
            meerkat_core::auth::PrincipalId::new("revoked-flow-operator").expect("principal");
        owner
            .mob_grant_scopes(
                &mob_id,
                principal.clone(),
                BTreeSet::from([ControlScope::SendCommand]),
                None,
            )
            .await
            .expect("grant SendCommand");
        let workgraph =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        let operator = Arc::new(
            MobMcpState::new(svc, MobControlPrincipal::External(principal.clone()))
                .expect("construct runtime authority")
                .with_workgraph_service(Some(workgraph.clone())),
        );
        operator
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;
        let config = <MobMcpState as WorkGraphFlowHost>::admit_workgraph_flow(
            operator.as_ref(),
            &mob_id,
            &FlowId::from("demo"),
        )
        .await
        .expect("pre-admit launch authority");
        let item = workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Revoked launch recovery".to_string(),
                ..Default::default()
            })
            .await
            .expect("item");
        let binding_id =
            meerkat::WorkExecutionBindingId::new("execution-revoked-launch").expect("binding id");
        let target = meerkat::WorkExecutionTarget::mob_flow(
            mob_id.to_string(),
            "demo",
            config.config.definition_digest().expect("config digest"),
            "369f4dfb-0b67-5ecf-806f-eaf3c0177291",
            meerkat::WorkExecutionAuthority::principal(principal.clone()),
            json!({}),
        )
        .expect("target");
        let (machine_state, _) = meerkat::WorkExecutionMachine::bind(&binding_id, target.run_id())
            .expect("bind machine");
        let bound = workgraph
            .execution_bridge()
            .bind_execution(
                meerkat::WorkExecutionBinding {
                    binding_id: binding_id.clone(),
                    work_ref: meerkat::WorkItemRef {
                        realm_id: item.realm_id.clone(),
                        namespace: item.namespace.clone(),
                        item_id: item.id.clone(),
                    },
                    target,
                    idempotency_key: "revoked-launch".to_string(),
                    correlation_id: "2d26bcba-10f6-5626-a2a5-4d1897b88d8b".to_string(),
                    supersedes: None,
                    machine_state,
                    created_at: workgraph
                        .store()
                        .get_store_time_utc()
                        .await
                        .expect("store time"),
                },
                item.revision,
            )
            .await
            .expect("commit binding before launch")
            .binding;
        owner
            .mob_revoke_scopes(
                &mob_id,
                principal,
                Some(BTreeSet::from([ControlScope::SendCommand])),
            )
            .await
            .expect("revoke SendCommand");

        let error = WorkGraphFlowBridge::new(operator.as_ref())
            .reconcile_workgraph_flow(
                Some(item.realm_id.clone()),
                Some(item.namespace.clone()),
                bound.binding_id.clone(),
            )
            .await
            .expect_err("revoked authority becomes terminal launch failure");
        assert!(matches!(
            error,
            WorkGraphFlowBridgeError::LaunchFailed { .. }
        ));
        let terminal = workgraph
            .execution_binding(
                Some(item.realm_id.clone()),
                Some(item.namespace.clone()),
                bound.binding_id,
            )
            .await
            .expect("terminal binding");
        assert!(matches!(
            meerkat::WorkExecutionMachine::recover_effect(&terminal).expect("terminal effect"),
            meerkat::WorkExecutionLifecycleEffect::LaunchFailed { .. }
        ));
        let item = workgraph
            .get(Some(item.realm_id), Some(item.namespace), item.id)
            .await
            .expect("item with launch failure evidence");
        assert!(item.evidence_refs.iter().any(|evidence| {
            evidence.id == terminal.evidence_id() && evidence.kind == "mob_flow_launch_failed"
        }));
    }

    #[tokio::test]
    async fn test_mcp_flow_status_rejects_invalid_run_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": "does-not-matter",
                "run_id": "not-a-uuid"
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "invalid run_id should map to invalid arguments"
        );
    }

    #[tokio::test]
    #[ignore = "requires live comms peer after external binding validation was added"]
    async fn test_mob_spawn_backend_arg_returns_backend_member_ref() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": {
                    "id": "ext-mob",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-8",
                            "tools": {"comms": true},
                            "external_addressable": true
                        },
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": {"comms": true},
                            "external_addressable": false
                        }
                    },
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {
                        "default": "session",
                        "external": {"address_base": "https://backend.example.invalid/mesh"}
                    }
                }
            }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{
                    "profile": "worker",
                    "agent_identity": "w-ext",
                    "binding": {
                        "kind": "external",
                        "address": "inproc://test-w-ext",
                        "identity": {
                            "kind": "ed25519_public_key",
                            "public_key": ED25519_PUBLIC_KEY_7
                        }
                    }
                }]
            }),
        )
        .await;
        let row = &spawned["results"].as_array().expect("typed spawn results")[0];
        assert_eq!(row["status"], "spawned");
        assert_eq!(row["result"]["agent_identity"], "w-ext");
        assert!(row["result"]["member_ref"].is_string());
        assert!(row.get("ok").is_none());
    }

    #[tokio::test]
    async fn test_mob_spawn_runtime_mode_defaults_and_override() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-spawn-runtime-mode-defaults-and-override","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "agent_identity": "lead-default"}]
            }),
        )
        .await;
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "agent_identity": "worker-turn", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;

        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        let members = listed["members"].as_array().cloned().unwrap_or_default();
        let lead_mode = members
            .iter()
            .find(|m| m["agent_identity"] == "lead-default")
            .and_then(|m| m["runtime_mode"].as_str());
        let worker_mode = members
            .iter()
            .find(|m| m["agent_identity"] == "worker-turn")
            .and_then(|m| m["runtime_mode"].as_str());

        assert_eq!(lead_mode, Some("autonomous_host"));
        assert_eq!(worker_mode, Some("turn_driven"));
    }

    /// A parked keep-alive turn must not hold the mock's keep-alive notifier
    /// map. `test_mob_spawn_many_dispatches_batch` hung in about 1% of loaded
    /// runs: the first autonomous member's host turn parked inside an `if let`
    /// whose scrutinee read guard stayed alive, and the second member's create
    /// then waited forever for that map's write lock. Deterministic: the turn
    /// is polled once (it takes the map and parks), and the next keep-alive
    /// create must then complete on its first poll.
    #[tokio::test]
    async fn mock_keep_alive_turn_parks_without_holding_the_notifier_map() {
        use futures::FutureExt;

        let svc = MockSessionSvc::new();
        let keep_alive = || CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                keep_alive: true,
                ..Default::default()
            }),
            labels: None,
        };
        let first = svc
            .create_session(keep_alive())
            .await
            .expect("create the first keep-alive session");
        let mut turn = std::pin::pin!(svc.start_turn(
            &first.session_id,
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: "host loop".to_string().into(),
                system_prompt: None,
                event_tx: None,
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    HandlingMode::Queue,
                    None,
                    None,
                ),
            },
        ));
        assert!(
            futures::poll!(&mut turn).is_pending(),
            "a keep-alive turn parks until it is interrupted"
        );
        let second = svc
            .create_session(keep_alive())
            .now_or_never()
            .expect("a parked keep-alive turn must not block the next keep-alive create")
            .expect("create the second keep-alive session");
        assert_ne!(second.session_id, first.session_id);
        svc.interrupt(&first.session_id)
            .await
            .expect("interrupt the parked turn");
        turn.await.expect("the interrupted keep-alive turn returns");
    }

    #[tokio::test]
    async fn test_mob_spawn_many_dispatches_batch() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-spawn-many-dispatches-batch","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"worker","agent_identity":"w-many-a"},
                    {"profile":"worker","agent_identity":"w-many-b"}
                ]
            }),
        )
        .await;
        let results = spawned["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected two batch rows");
        assert!(
            results
                .iter()
                .all(|row| row["status"] == json!("spawned")
                    && row["result"]["member_ref"].is_string()),
            "all batch spawn rows should succeed"
        );

        let listed = call_tool(&d, "mob_list_members", json!({"mob_id": mob_id})).await;
        let ids = listed["members"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m["agent_identity"].as_str().map(ToString::to_string))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains("w-many-a"));
        assert!(ids.contains("w-many-b"));
    }

    #[tokio::test]
    async fn test_mob_spawn_many_dispatches_typed_failure_cause() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-spawn-many-dispatches-typed-failure-cause","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"missing","agent_identity":"w-missing"}
                ]
            }),
        )
        .await;
        let row = &spawned["results"].as_array().expect("results array")[0];
        assert_eq!(row["status"], "failed");
        assert_eq!(row["result"]["cause"], "profile_not_found");
        assert!(
            row["result"]["message"].as_str().is_some_and(|msg| {
                msg.contains("profile not found") && msg.contains("missing")
            })
        );
        assert!(row.get("ok").is_none());
        assert!(row.get("error").is_none());
    }

    #[tokio::test]
    async fn test_mob_wait_kickoff_returns_member_snapshots() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-wait-kickoff-returns-member-snapshots","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"lead","agent_identity":"lead-kickoff","runtime_mode":"turn_driven"},
                    {"profile":"worker","agent_identity":"worker-kickoff","runtime_mode":"turn_driven"}
                ]
            }),
        )
        .await;

        let waited = call_tool(
            &d,
            "mob_wait_kickoff",
            json!({
                "mob_id": mob_id,
                "member_ids": ["lead-kickoff", "worker-kickoff"],
                "timeout_ms": 2000
            }),
        )
        .await;
        let members = waited["members"].as_array().expect("members array");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["agent_identity"], "lead-kickoff");
        assert_eq!(members[1]["agent_identity"], "worker-kickoff");
    }

    #[tokio::test]
    async fn test_bound_mob_wait_ready_returns_detached_operation() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(Arc::clone(&state));

        let created = call_tool(
            &d,
            "mob_create",
            json!({"definition":{"id":"test_ready_detached","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new());
        let dispatcher = Arc::new(MobMcpDispatcher::new(state));
        let bound = AgentToolDispatcher::bind_ops_lifecycle(
            dispatcher,
            Arc::clone(&registry),
            SessionId::new(),
        )
        .expect("dispatcher should bind ops lifecycle")
        .into_dispatcher();

        let args = json!({
            "mob_id": mob_id,
            "timeout_ms": 60_000
        });
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        let outcome = bound
            .dispatch(mk_call("mob_wait_ready", &raw))
            .await
            .expect("bound wait_ready dispatch should return detached op");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("tool json");

        assert_eq!(payload["status"], "waiting");
        assert_eq!(payload["wait_policy"], "detached");
        assert!(payload.get("members").is_none());
        assert_eq!(outcome.async_ops.len(), 1);
        assert_eq!(
            outcome.async_ops[0].wait_policy,
            meerkat_core::ops::WaitPolicy::Detached
        );
        assert_eq!(
            payload["operation_id"].as_str(),
            Some(outcome.async_ops[0].operation_id.to_string().as_str())
        );
        assert!(
            registry
                .snapshot(&outcome.async_ops[0].operation_id)
                .expect("registry snapshot should succeed")
                .is_some(),
            "detached wait operation should be registered before dispatch returns"
        );
    }

    #[tokio::test]
    async fn test_mob_wait_kickoff_completes_after_initial_turn() {
        // The kickoff barrier waits for the initial autonomous turn to complete.
        // Use turn_driven members to avoid the keep_alive mock blocking.
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"test_mob-wait-kickoff-completes-after-initial-turn","orchestrator":{"profile":"lead"},"profiles":{"lead":{"model":"claude-opus-4-8","external_addressable":true,"tools":{"comms":true}},"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        call_tool(
            &d,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"lead","agent_identity":"kickoff-lead","runtime_mode":"turn_driven"},
                    {"profile":"worker","agent_identity":"kickoff-worker","runtime_mode":"turn_driven"}
                ]
            }),
        )
        .await;

        // Turn-driven members don't run an autonomous kickoff turn,
        // so the barrier returns immediately.
        let waited = call_tool(
            &d,
            "mob_wait_kickoff",
            json!({
                "mob_id": mob_id,
                "member_ids": ["kickoff-lead", "kickoff-worker"],
                "timeout_ms": 2000
            }),
        )
        .await;
        let members = waited["members"].as_array().expect("members array");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["agent_identity"], "kickoff-lead");
        assert_eq!(members[1]["agent_identity"], "kickoff-worker");
    }

    #[tokio::test]
    async fn test_mob_create_rejects_duplicate_mob_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"definition":{"id":"dup_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        let error = call_tool_err(&d, "mob_create", json!({"definition":{"id":"dup_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})).await;
        assert!(
            matches!(error, ToolError::ExecutionFailed { .. }),
            "duplicate mob creation should fail deterministically"
        );

        let listed = call_tool(&d, "mob_list", json!({})).await;
        let ids: Vec<String> = listed["mobs"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|m| m["mob_id"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![mob_id],
            "duplicate create must not replace active mob"
        );
    }

    #[tokio::test]
    async fn test_mobpack_duplicate_create_requires_same_verified_identity() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let definition = explicit_definition("dup-pack-mob");
        let first_identity =
            meerkat_mob::MobDefinitionSourceIdentity::mobpack("a".repeat(64), Vec::new());
        let second_identity =
            meerkat_mob::MobDefinitionSourceIdentity::mobpack("b".repeat(64), Vec::new());

        let mob_id = state
            .mob_create_from_mobpack(definition.clone(), BTreeMap::new(), first_identity.clone())
            .await
            .expect("initial mobpack create");
        assert_eq!(mob_id.as_str(), "dup-pack-mob");

        let same = state
            .mob_create_from_mobpack(definition.clone(), BTreeMap::new(), first_identity)
            .await
            .expect("same verified mobpack identity should be idempotent");
        assert_eq!(same, mob_id);

        let err = state
            .mob_create_from_mobpack(definition, BTreeMap::new(), second_identity)
            .await
            .expect_err("mismatched verified mobpack digest must fail closed");
        assert!(
            err.to_string().contains("verified pack identity"),
            "expected verified identity mismatch error, got {err}"
        );
    }

    /// A definition whose inline profile opts into a tool bundle.
    fn opting_definition(mob_id: &str) -> MobDefinition {
        let mut definition = explicit_definition(mob_id);
        for binding in definition.profiles.values_mut() {
            if let Some(profile) = binding.as_inline_mut() {
                profile.tools.bundles = vec!["announcements".to_string()];
            }
        }
        definition
    }

    /// A create that is not a child mob's offers no tool bundle: a host create
    /// and a mobpack create naming one are refused typed before anything of
    /// the mob exists.
    #[tokio::test]
    async fn test_host_and_mobpack_creates_offer_no_tool_bundle() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let refusal = |error: MobError| match error {
            MobError::ToolBundleNotOffered {
                profile,
                bundle,
                offered,
            } => (profile.to_string(), bundle, offered),
            other => panic!("a typed bundle refusal, got {other:?}"),
        };
        let error = state
            .mob_create_definition(opting_definition("opting-host-mob"))
            .await
            .expect_err("a host create offers no bundle");
        assert_eq!(
            refusal(error),
            (
                "worker".to_string(),
                "announcements".to_string(),
                Vec::new()
            )
        );
        let error = state
            .mob_create_from_mobpack(
                opting_definition("opting-pack-mob"),
                BTreeMap::new(),
                meerkat_mob::MobDefinitionSourceIdentity::mobpack("c".repeat(64), Vec::new()),
            )
            .await
            .expect_err("a mobpack create offers no bundle");
        assert_eq!(
            refusal(error),
            (
                "worker".to_string(),
                "announcements".to_string(),
                Vec::new()
            )
        );
        for mob_id in ["opting-host-mob", "opting-pack-mob"] {
            assert!(
                state.handle_for(&MobId::from(mob_id)).await.is_err(),
                "no mob is created"
            );
        }
    }

    #[tokio::test]
    async fn test_mob_create_rejects_missing_definition() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        // definition field is required at the serde level — empty args should
        // fail at parse_args() time with InvalidArguments.
        let error = call_tool_err(&d, "mob_create", json!({})).await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "mob_create with missing definition must return InvalidArguments, got: {error:?}"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_invalid_definition() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        // A definition with no profiles triggers DiagnosticCode::EmptyProfiles
        // in validate_definition() and must surface as ExecutionFailed.
        let error = call_tool_err(
            &d,
            "mob_create",
            json!({"definition": {"id": "bad_mob", "profiles": {}}}),
        )
        .await;
        assert!(
            matches!(error, ToolError::ExecutionFailed { .. }),
            "mob_create with empty profiles must return ExecutionFailed, got: {error:?}"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_internal_profile_tool_bundles() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_create",
            json!({
                "definition": {
                    "id": "bad_mob",
                    "profiles": {
                        "worker": {
                            "model": "claude-sonnet-4-6",
                            "tools": {
                                "rust_bundles": ["internal-only"]
                            }
                        }
                    }
                }
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "mob_create with internal rust bundle fields must return InvalidArguments, got: {error:?}"
        );
    }

    // ── Implicit mob methods ──────────────────────────────────────────

    #[tokio::test]
    async fn test_find_implicit_mob_returns_none_when_no_implicit_mob() {
        let state = MobMcpState::new_in_memory();
        assert!(
            state
                .find_implicit_mob_for_bridge_session("nonexistent")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_or_create_implicit_mob_creates_and_reuses() {
        let state = MobMcpState::new_in_memory();
        let session_id = SessionId::new();
        let sid = session_id.to_string();

        let mob_id_1 = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        let mob_id_2 = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert_eq!(mob_id_1, mob_id_2, "second call must return same mob_id");

        let found = state.find_implicit_mob_for_bridge_session(&sid).await;
        assert_eq!(found, Some(mob_id_1));
    }

    #[tokio::test]
    async fn test_get_or_create_implicit_mob_distinct_sessions() {
        let state = MobMcpState::new_in_memory();
        let sid_a = SessionId::new().to_string();
        let sid_b = SessionId::new().to_string();

        let mob_a = state
            .get_or_create_implicit_mob_for_bridge_session(&sid_a, "claude-sonnet-4-5")
            .await
            .unwrap();
        let mob_b = state
            .get_or_create_implicit_mob_for_bridge_session(&sid_b, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert_ne!(mob_a, mob_b, "different sessions must get different mobs");
    }

    #[tokio::test]
    async fn in_memory_state_starts_local_capability_sweeper() {
        let state = MobMcpState::new_in_memory();
        state
            .ensure_restored()
            .await
            .expect("in-memory restoration is a no-op");
        assert!(
            state
                .local_forked_participant_sweeper_started
                .load(std::sync::atomic::Ordering::SeqCst),
            "in-memory states still own local capability convergence"
        );
    }

    #[tokio::test]
    async fn test_ensure_implicit_mob_for_model_reconciles_stale_model_in_state() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let old_mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .expect("create initial implicit mob");
        let old_handle = state
            .handle_for(&old_mob_id)
            .await
            .expect("initial implicit mob handle");
        assert_eq!(
            old_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .as_inline()
                .unwrap()
                .model,
            "claude-sonnet-4-5"
        );

        let (new_mob_id, created) = state
            .ensure_implicit_mob_for_model(&sid, "gpt-5.4", Some(&old_mob_id))
            .await
            .expect("reconcile implicit mob");

        assert!(created, "model mismatch should force a fresh implicit mob");
        assert_eq!(
            state.find_implicit_mob_for_bridge_session(&sid).await,
            Some(new_mob_id.clone()),
            "session should now point at the reconciled implicit mob"
        );
        assert_eq!(
            new_mob_id, old_mob_id,
            "implicit mob IDs are canonical per session even when the runtime refreshes their model"
        );
        let new_handle = state
            .handle_for(&new_mob_id)
            .await
            .expect("reconciled implicit mob handle");
        assert_eq!(
            new_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .as_inline()
                .unwrap()
                .model,
            "gpt-5.4"
        );
    }

    #[tokio::test]
    async fn test_is_implicit_mob_true_for_implicit_false_for_explicit() {
        let state = MobMcpState::new_in_memory();

        // Create an implicit mob
        let sid = SessionId::new().to_string();
        let implicit_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();

        // Create an explicit mob via mob_create_definition
        let explicit_id = state
            .mob_create_definition(explicit_definition("explicit-mob"))
            .await
            .unwrap();

        assert!(state.is_implicit_mob(&implicit_id).await);
        assert!(!state.is_implicit_mob(&explicit_id).await);
        assert!(!state.is_implicit_mob(&MobId::from("nonexistent")).await);
    }

    fn explicit_definition(mob_id: &str) -> MobDefinition {
        let mut explicit_profiles = BTreeMap::new();
        explicit_profiles.insert(
            ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::profile::Profile {
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::profile::ToolConfig {
                    comms: true,
                    ..meerkat_mob::profile::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
                durable: false,
            })),
        );
        let mut definition = MobDefinition::explicit(MobId::from(mob_id));
        definition.profiles = explicit_profiles;
        definition
    }

    struct TestForkedParticipantSource;

    #[async_trait]
    impl meerkat_mob::ForkedParticipantSourceRuntime for TestForkedParticipantSource {
        async fn session_execution_evidence(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<meerkat_mob::forked_participant::SessionExecutionEvidence>, SessionError>
        {
            Ok(Some(
                meerkat_mob::forked_participant::SessionExecutionEvidence {
                    agent_identity: Some(AgentIdentity::from("source-member")),
                    realm_id: Some(
                        meerkat_core::RealmId::parse("global").expect("valid test realm"),
                    ),
                    tool_access_policy: None,
                    auth_binding: None,
                },
            ))
        }

        async fn fork_planned_child(
            &self,
            request: meerkat_mob::forked_participant::PlannedForkRequest,
        ) -> Result<meerkat_mob::forked_participant::PlannedForkOutcome, SessionError> {
            Ok(meerkat_mob::forked_participant::PlannedForkOutcome {
                child_session_id: request.planned_child_session_id,
                prefix_message_count: request.prefix_message_count.unwrap_or(0),
                prefix_digest: "test-prefix-digest".to_string(),
            })
        }

        async fn planned_child_evidence(
            &self,
            _child_session_id: &SessionId,
        ) -> Result<Option<meerkat_mob::forked_participant::PlannedChildEvidence>, SessionError>
        {
            Ok(None)
        }

        async fn archive_fork_session(
            &self,
            _child_session_id: &SessionId,
        ) -> Result<(), SessionError> {
            Ok(())
        }
    }

    async fn mint_local_forked_participant(
        state: &MobMcpState,
        request_id: &str,
        reuse: meerkat_mob::forked_participant::ForkedParticipantReusePolicy,
    ) -> meerkat_mob::forked_participant::ForkedParticipantRef {
        let realm_id = meerkat_core::RealmId::parse("global").expect("valid test realm");
        let service = meerkat_mob::forked_participant::ForkedParticipantService::new(
            meerkat_mob::forked_participant::ForkedParticipantOwnerRoute::Local {
                realm_id: realm_id.clone(),
            },
            state.forked_participant_store().clone(),
            Arc::new(TestForkedParticipantSource),
        )
        .expect("compose source capability service");
        service
            .create(
                &meerkat_mob::forked_participant::ForkedParticipantRequest {
                    request_id: meerkat_mob::forked_participant::ForkedParticipantRequestId::new(
                        request_id,
                    )
                    .expect("valid request id"),
                    source_identity: AgentIdentity::from("source-member"),
                    source_session_id: SessionId::new(),
                    owner_route:
                        meerkat_mob::forked_participant::ForkedParticipantOwnerRoute::Local {
                            realm_id,
                        },
                    prefix_message_count: None,
                    scope:
                        meerkat_mob::forked_participant::ForkedParticipantOperationScope::InvokeAndObserve,
                    reuse,
                    ttl: Duration::from_secs(600),
                },
                chrono::Utc::now(),
            )
            .await
            .expect("mint source capability")
    }

    #[tokio::test]
    async fn registry_mobs_share_in_memory_forked_participant_custody() {
        let state = MobMcpState::new_in_memory();
        let source_mob = state
            .mob_create_definition(explicit_definition("capability-source"))
            .await
            .expect("create source mob");
        let target_mob = state
            .mob_create_definition(explicit_definition("capability-target"))
            .await
            .expect("create target mob");
        let source_storage = state
            .storage_for_new_mob(&source_mob)
            .await
            .expect("compose source storage")
            .0;
        let target_storage = state
            .storage_for_new_mob(&target_mob)
            .await
            .expect("compose target storage")
            .0;
        assert!(Arc::ptr_eq(
            source_storage
                .forked_participant_store()
                .expect("source capability store"),
            target_storage
                .forked_participant_store()
                .expect("target capability store"),
        ));
        let capability = mint_local_forked_participant(
            &state,
            "cross-mob-capability",
            meerkat_mob::forked_participant::ForkedParticipantReusePolicy::BoundedReuse {
                max_uses: 2,
            },
        )
        .await;

        // Destroying the source mob only removes its event store. The
        // state-owned custody remains available to another managed mob.
        state
            .mob_destroy(&source_mob)
            .await
            .expect("destroy source mob");
        let attachment_id =
            meerkat_mob::forked_participant::ForkedParticipantAttachmentId::new("target-attach")
                .expect("valid attachment id");
        let target_service = meerkat_mob::forked_participant::ForkedParticipantService::new(
            capability.owner_route().clone(),
            state.forked_participant_store().clone(),
            Arc::new(TestForkedParticipantSource),
        )
        .expect("compose target capability service");
        let grant = target_service
            .attach(&capability, &attachment_id, true, chrono::Utc::now())
            .await
            .expect("target must load and attach source capability");
        assert_eq!(grant.attachment_id, attachment_id);
        assert!(
            state
                .forked_participant_store()
                .load_exact(&capability)
                .await
                .is_ok(),
            "the state-owned store retains the exact protected reference"
        );
    }

    #[tokio::test]
    async fn persistent_registry_capability_custody_survives_mob_and_state_restart() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .try_with_persistent_storage_root(Some(root.path().to_path_buf()))
                .expect("open rooted capability custody"),
        );
        let source_mob = state
            .mob_create_definition(explicit_definition("persistent-capability-source"))
            .await
            .expect("create source mob");
        let target_mob = state
            .mob_create_definition(explicit_definition("persistent-capability-target"))
            .await
            .expect("create target mob");
        let capability = mint_local_forked_participant(
            &state,
            "persistent-cross-mob-capability",
            meerkat_mob::forked_participant::ForkedParticipantReusePolicy::OneShot,
        )
        .await;

        let custody_path = MobMcpState::persistent_mob_root(root.path())
            .join(MobMcpState::REALM_FORKED_PARTICIPANT_STORE_FILE_NAME);
        assert!(
            tokio::fs::metadata(&custody_path).await.is_ok(),
            "realm custody must have an explicitly rooted durable database"
        );
        state
            .mob_destroy(&source_mob)
            .await
            .expect("destroy source mob without deleting realm custody");
        assert!(
            tokio::fs::metadata(&custody_path).await.is_ok(),
            "source destruction must not delete shared capability custody"
        );

        state
            .handle_for(&target_mob)
            .await
            .expect("target mob handle")
            .shutdown()
            .await
            .expect("shutdown target for state restart");
        drop(state);

        let restored = MobMcpState::new(svc, MobControlPrincipal::Owner)
            .expect("construct runtime authority")
            .try_with_persistent_storage_root(Some(root.path().to_path_buf()))
            .expect("reopen rooted capability custody");
        assert!(
            restored
                .forked_participant_store()
                .load_exact(&capability)
                .await
                .is_ok(),
            "state restart must reopen the same durable capability record"
        );
        assert_eq!(
            restored
                .mob_list()
                .await
                .expect("restore managed mobs")
                .len(),
            1,
            "only the surviving target mob is restored; custody is not a mob"
        );
    }

    #[tokio::test]
    async fn realm_states_and_direct_mob_storage_remain_isolated() {
        let first = MobMcpState::new_in_memory();
        let second = MobMcpState::new_in_memory();
        assert!(
            !Arc::ptr_eq(
                first.forked_participant_store(),
                second.forked_participant_store()
            ),
            "separate state instances are separate realm capability custodies"
        );

        let direct_first = MobStorage::in_memory();
        let direct_second = MobStorage::in_memory();
        let _first_builder =
            MobBuilder::new(explicit_definition("direct-first"), direct_first.clone());
        let _second_builder =
            MobBuilder::new(explicit_definition("direct-second"), direct_second.clone());
        assert!(
            !Arc::ptr_eq(
                direct_first
                    .forked_participant_store()
                    .expect("direct storage capability store"),
                direct_second
                    .forked_participant_store()
                    .expect("direct storage capability store"),
            ),
            "direct MobBuilder storage remains local unless its caller injects a store"
        );
    }

    fn sample_realm_profile(model: &str) -> meerkat_mob::Profile {
        meerkat_mob::profile::Profile {
            model_fallback: None,
            model: model.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: meerkat_mob::profile::ToolConfig::default(),
            peer_description: "realm worker".to_string(),
            external_addressable: false,
            backend: None,
            runtime_mode: meerkat_mob::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        }
    }

    #[tokio::test]
    async fn realm_profile_child_mobs_hydrate_seeded_skill_sources() {
        let svc = Arc::new(MockSessionSvc::new());
        let store = Arc::new(meerkat_mob::InMemoryRealmProfileStore::new())
            as Arc<dyn meerkat_mob::RealmProfileStore>;
        let mut profile = sample_realm_profile("gpt-5.5");
        profile.skills = vec!["ops-investigation-worker".to_string()];
        store
            .create("investigation-worker", &profile)
            .await
            .expect("realm profile seeded");

        let mut sources = BTreeMap::new();
        sources.insert(
            "ops-investigation-worker".to_string(),
            SkillSource::Inline {
                content: "investigation worker rules".to_string(),
            },
        );

        let state = MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
            .expect("construct runtime authority")
            .with_realm_profile_store(Some(store))
            .with_realm_skill_sources(sources);
        let mut definition = MobDefinition::explicit(MobId::from("child-mob"));
        definition.profiles.insert(
            ProfileName::from("investigation-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "investigation-worker".to_string(),
            },
        );

        state
            .hydrate_definition_skill_sources(&mut definition)
            .await
            .expect("hydration succeeds");

        let SkillSource::Inline { content } = definition
            .skills
            .get("ops-investigation-worker")
            .expect("seeded source copied")
        else {
            panic!("expected inline seeded skill source");
        };
        assert_eq!(content, "investigation worker rules");
    }

    #[tokio::test]
    async fn realm_ref_mob_create_spawns_against_shared_profile_store() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
            .expect("construct runtime authority");
        let mut profile = sample_realm_profile("gpt-5.5");
        profile.tools.comms = true;
        state
            .realm_profile_create("investigation-worker", &profile)
            .await
            .expect("realm profile seeded");
        state
            .realm_profile_create("person-worker", &profile)
            .await
            .expect("person profile seeded");

        let mut definition = MobDefinition::explicit(MobId::from("nest-1779406812080"));
        definition.wiring.auto_wire_orchestrator = true;
        definition.profiles.insert(
            ProfileName::from("investigation-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "investigation-worker".to_string(),
            },
        );
        definition.profiles.insert(
            ProfileName::from("person-worker"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "person-worker".to_string(),
            },
        );

        let mob_id = state
            .mob_create_definition(definition)
            .await
            .expect("realm-ref mob create should succeed");
        let mut spec = SpawnMemberSpec::new(
            ProfileName::from("investigation-worker"),
            AgentIdentity::from("investigation-worker-nest-1779406812080"),
        );
        spec.auto_wire_parent = true;

        let result = state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .expect("realm-ref profile should resolve from shared state store");
        assert_eq!(
            result.agent_identity,
            AgentIdentity::from("investigation-worker-nest-1779406812080")
        );
    }

    #[tokio::test]
    async fn test_persistent_root_restores_explicit_mob_member_status() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("restored-explicit"))
            .await
            .expect("create explicit mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");

        // A RESTART MEANS THE PREDECESSOR IS GONE, and this test has to actually
        // establish that. It used to build the second state while the first
        // mob's supervisor route was still published, so it was not simulating a
        // restart at all - it was simulating two live holders of one supervisor
        // name and relying on the second to take the name over. The one claim
        // rule (see `MobSupervisorBridge` in meerkat-mob) never takes over a
        // live route, "whether the holder is another mob sharing this mob id or
        // an earlier generation of this same authority", and it names the
        // sanctioned succession: `shutdown()` on the predecessor, which reaches
        // a generation-exact `retire_inproc_route` and frees the name.
        //
        // Retiring it here is what makes the assertions below about rehydration
        // FROM DISK rather than about name displacement. Sibling restore tests
        // pass only because they build their second state before anything
        // publishes a supervisor route.
        state
            .handle_for(&mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");

        let restored = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let status = restored
            .mob_member_status(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("restore member status");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
        // Verify the member has a live bridge-session binding via the handle.
        let handle = restored.handle_for(&mob_id).await.expect("mob handle");
        assert!(
            handle
                .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
                .await
                .is_some(),
            "restored member should still have a live bridge-session binding"
        );

        let mobs = restored.mob_list().await.expect("restore mob list");
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].0, mob_id);
    }

    /// A first access cancelled while its restore's owned resume is parked
    /// before registration leaves the restore incomplete; the resume then
    /// registers the mob without its caller, and the next access completes
    /// the restore around that exact registered store instead of resuming it
    /// a second time (which the incumbent supervisor would refuse).
    #[tokio::test]
    async fn a_restore_cancelled_mid_resume_completes_on_the_next_access() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let mob_id = state
            .mob_create_definition(explicit_definition("restore-cancelled"))
            .await
            .expect("create explicit mob");
        state
            .handle_for(&mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");

        let restored = MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
            .expect("construct runtime authority")
            .with_persistent_storage_root(Some(root.path().to_path_buf()))
            .into_shared();
        let hold = Arc::new(InsertHold::default());
        *restored
            .resume_insert_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hold));
        let first = {
            let restored = Arc::clone(&restored);
            tokio::spawn(async move { restored.mob_list().await })
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the restore's resume reaches its registration seam");
        first.abort();
        assert!(
            first
                .await
                .expect_err("the first access was cancelled")
                .is_cancelled()
        );

        *restored
            .resume_insert_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        hold.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !restored.mobs.read().await.contains_key(&mob_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the owned resume registers the mob without its caller");

        let mobs = restored
            .mob_list()
            .await
            .expect("the next access completes the restore");
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].0, mob_id);
        restored
            .handle_for(&mob_id)
            .await
            .expect("the one restored mob");
    }

    /// A host spawn customizer that widens every member's application tool
    /// policy to Unmanaged, recording each call: the mob, the spawn source,
    /// the member and, on a resume, the session of its resume view.
    #[derive(Default)]
    struct WideningHostCustomizer {
        calls: std::sync::Mutex<
            Vec<(
                MobId,
                meerkat_mob::SpawnSource,
                AgentIdentity,
                Option<SessionId>,
            )>,
        >,
    }

    impl WideningHostCustomizer {
        fn record(
            &self,
            ctx: &meerkat_mob::SpawnCustomizationContext,
            spec: &mut SpawnMemberSpec,
            view: Option<SessionId>,
        ) {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    ctx.mob_id.clone(),
                    ctx.spawn_source,
                    spec.identity.clone(),
                    view,
                ));
            spec.application_tool_policy =
                Some(meerkat_core::ApplicationToolPolicyBinding::Unmanaged);
        }

        fn calls(
            &self,
        ) -> Vec<(
            MobId,
            meerkat_mob::SpawnSource,
            AgentIdentity,
            Option<SessionId>,
        )> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl meerkat_mob::SpawnMemberCustomizer for WideningHostCustomizer {
        fn customize_spawn(
            &self,
            ctx: &meerkat_mob::SpawnCustomizationContext,
            spec: &mut SpawnMemberSpec,
        ) -> Result<(), MobError> {
            self.record(ctx, spec, None);
            Ok(())
        }

        fn customize_resume(
            &self,
            ctx: &meerkat_mob::SpawnCustomizationContext,
            durable: &meerkat_mob::ResumedMemberView,
            spec: &mut SpawnMemberSpec,
        ) -> Result<(), MobError> {
            self.record(ctx, spec, Some(durable.session_id.clone()));
            Ok(())
        }
    }

    fn managed_policy_registry() -> Arc<meerkat_core::ToolConsequencePolicyRegistry> {
        Arc::new(
            meerkat_core::ToolConsequencePolicyRegistry::new(
                Vec::new(),
                meerkat_core::PolicyEvaluationSupervisorConfig::default(),
                None,
            )
            .expect("policy registry"),
        )
    }

    fn host_child_policy() -> meerkat_core::ApplicationToolPolicyBinding {
        meerkat_core::ApplicationToolPolicyBinding::Provider {
            provider_id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
            policy_id: meerkat_core::PolicyId::new("child-tools").expect("policy id"),
        }
    }

    /// #1701: the host's spawn customizer reaches the members of every mob the
    /// state builds; in a child mob the child policy runs after it, so a host
    /// customizer that widens the application tool policy is overridden. In a
    /// host-created mob the child policy does not govern, and the host's
    /// choice stands.
    #[tokio::test]
    async fn host_spawn_customizer_runs_first_and_the_child_policy_has_the_last_word() {
        let svc = Arc::new(MockSessionSvc::new());
        let host = Arc::new(WideningHostCustomizer::default());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_tool_consequence_policy_registry(managed_policy_registry())
                .with_child_application_tool_policy(host_child_policy())
                .with_child_spawn_member_customizer(host.clone()),
        );
        let child = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("customized-child"),
                SessionId::new(),
                true,
                false,
            )
            .await
            .expect("create child mob");
        let root = state
            .mob_create_definition(explicit_definition("customized-root"))
            .await
            .expect("create host mob");
        let worker = AgentIdentity::from("worker-1");
        let mut sessions = Vec::new();
        for mob_id in [&child, &root] {
            state
                .mob_spawn(
                    mob_id,
                    ProfileName::from("worker"),
                    worker.clone(),
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                    None,
                )
                .await
                .expect("spawn worker");
            sessions.push(
                state
                    .mob_resolve_bridge_session_id(mob_id, &worker)
                    .await
                    .expect("resolve bridge session")
                    .expect("worker bridge session"),
            );
        }
        assert_eq!(
            svc.build_policies_for(&sessions[0]),
            [host_child_policy()],
            "the child policy overrides the host customizer's widening"
        );
        assert_eq!(
            svc.build_policies_for(&sessions[1]),
            [meerkat_core::ApplicationToolPolicyBinding::Unmanaged],
            "outside child mobs the host customizer's choice stands"
        );
        let calls = host.calls();
        for mob_id in [&child, &root] {
            assert!(
                calls.iter().any(|(mob, source, member, view)| mob == mob_id
                    && *source != meerkat_mob::SpawnSource::Resume
                    && member == &worker
                    && view.is_none()),
                "the host customizer customizes the spawn into {mob_id}: {calls:?}"
            );
        }
    }

    /// Create a mob (a child mob when `child`) under `root`, seat `worker-1`
    /// and retire the state's supervisor route, as a process exit does.
    /// Returns the restarted session service (live sessions gone, persisted
    /// ones kept), the mob and the worker's session.
    async fn seat_worker_then_exit(
        svc: &Arc<MockSessionSvc>,
        root: &std::path::Path,
        child: bool,
        mob_name: &str,
    ) -> (Arc<MockSessionSvc>, MobId, SessionId) {
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.to_path_buf())),
        );
        let mob_id = if child {
            // A child mob outlives a restart only with its owner session.
            let owner = SessionId::new();
            svc.insert_persisted_session(Session::with_id(owner.clone()))
                .await;
            state
                .mob_create_definition_with_owner_bridge_session(
                    explicit_definition(mob_name),
                    owner,
                    true,
                    false,
                )
                .await
                .expect("create child mob")
        } else {
            state
                .mob_create_definition(explicit_definition(mob_name))
                .await
                .expect("create host mob")
        };
        let worker = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                worker.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let session = state
            .mob_resolve_bridge_session_id(&mob_id, &worker)
            .await
            .expect("resolve bridge session")
            .expect("worker bridge session");
        state
            .handle_for(&mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
        (Arc::new(svc.cold_restart().await), mob_id, session)
    }

    fn standing_state() -> Arc<MobMcpState> {
        let svc = Arc::new(MockSessionSvc::new());
        MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
            .expect("runtime custody")
            .into_shared()
    }

    fn far_deadline() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(10)
    }

    /// A mob whose Shutdown is draining closed its admission when the
    /// Shutdown was admitted: the close counts it closed, although its actor
    /// answers the close `ShutdownInProgress` instead of running it.
    #[tokio::test]
    async fn close_admission_counts_a_shutting_down_mob_as_closed() {
        let state = standing_state();
        let mob_id = state
            .mob_create_definition(explicit_definition("draining-mob-close"))
            .await
            .expect("create the mob");
        let handle = state.handle_for(&mob_id).await.expect("mob handle");
        let release = handle
            .hold_shutdown_drain_for_test()
            .await
            .expect("hold the next Shutdown's drain");
        let mut closed = handle.admission_closed();
        let shutdown_handle = handle.clone();
        let shutdown = tokio::spawn(async move { shutdown_handle.shutdown().await });
        tokio::time::timeout(Duration::from_secs(10), closed.wait_for(Option::is_some))
            .await
            .expect("the Shutdown is admitted")
            .expect("admission watch");

        assert_eq!(
            state.close_admission(far_deadline()).await,
            MobStateStandDown::Clean,
            "a draining mob's admission is already closed"
        );
        assert!(
            !shutdown.is_finished(),
            "the Shutdown still joins the held task"
        );

        drop(release);
        tokio::time::timeout(Duration::from_secs(10), shutdown)
            .await
            .expect("the Shutdown returns")
            .expect("join")
            .expect("the Shutdown finishes");
    }

    /// Owned work is joined, never aborted: a stand-down that cannot wait
    /// for it reports it unsettled and does not seal; once it finishes, a
    /// later stand-down is clean and seals the state.
    #[tokio::test]
    async fn stand_down_joins_owned_work_and_seals_only_when_it_finished() {
        let state = standing_state();
        let release = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (task_release, task_finished) = (Arc::clone(&release), Arc::clone(&finished));
        state
            .state_tasks
            .spawn(
                state_tasks::StateTaskKind::Owned,
                "test owned work",
                async move {
                    task_release.notified().await;
                    task_finished.store(true, std::sync::atomic::Ordering::SeqCst);
                },
            )
            .expect("an open state admits owned work");

        let early = state
            .stand_down(std::time::Instant::now() + Duration::from_millis(50))
            .await;
        assert!(
            matches!(&early, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("test owned work"))),
            "owned work still running is reported: {early:?}"
        );
        assert!(
            !finished.load(std::sync::atomic::Ordering::SeqCst),
            "owned work is never aborted"
        );
        assert!(
            state
                .state_tasks
                .spawn(
                    state_tasks::StateTaskKind::Periodic,
                    "late periodic",
                    async {}
                )
                .is_none(),
            "a closed state starts no periodic work"
        );

        release.notify_one();
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            state
                .state_tasks
                .spawn(state_tasks::StateTaskKind::Owned, "after seal", async {})
                .is_none(),
            "a sealed state admits nothing"
        );
        let refused = state
            .mob_create_definition(explicit_definition("after-stand-down"))
            .await
            .expect_err("a stood-down state admits no new mob");
        assert!(refused.to_string().contains("stood down"), "{refused}");
    }

    /// Periodic and convergent work is aborted, so it never holds a
    /// stand-down; owned work started while the stand-down runs (the second
    /// half of earlier admitted work) is admitted and joined too.
    #[tokio::test]
    async fn stand_down_aborts_loops_and_joins_late_owned_work() {
        let state = standing_state();
        for kind in [
            state_tasks::StateTaskKind::Periodic,
            state_tasks::StateTaskKind::Convergent,
        ] {
            state
                .state_tasks
                .spawn(kind, "test loop", std::future::pending::<()>())
                .expect("an open state admits background work");
        }
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = Arc::clone(&release);
        state
            .state_tasks
            .spawn(
                state_tasks::StateTaskKind::Owned,
                "first owned",
                async move {
                    task_release.notified().await;
                },
            )
            .expect("owned work admitted");

        let standing = Arc::clone(&state);
        let stand_down = tokio::spawn(async move { standing.stand_down(far_deadline()).await });
        // Let the stand-down close admission and start joining.
        while state.state_tasks.admits_new_work() {
            tokio::task::yield_now().await;
        }
        let late_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let late = Arc::clone(&late_finished);
        state
            .state_tasks
            .spawn(
                state_tasks::StateTaskKind::Owned,
                "late owned",
                async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    late.store(true, std::sync::atomic::Ordering::SeqCst);
                },
            )
            .expect("a closed but unsealed state still admits owned work");
        release.notify_one();
        let outcome = stand_down.await.expect("stand-down task");
        assert_eq!(outcome, MobStateStandDown::Clean);
        assert!(
            late_finished.load(std::sync::atomic::Ordering::SeqCst),
            "the late owned work was joined before the state sealed"
        );
    }

    /// The work stays in the state's custody while a stand-down waits for
    /// it: a stand-down that is cancelled mid-wait, or two that run at once,
    /// never report clean while owned work runs.
    #[tokio::test]
    async fn cancelled_and_concurrent_stand_downs_never_report_clean_early() {
        let state = standing_state();
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = Arc::clone(&release);
        state
            .state_tasks
            .spawn(
                state_tasks::StateTaskKind::Owned,
                "held owned work",
                async move {
                    task_release.notified().await;
                },
            )
            .expect("owned work admitted");

        let standing = Arc::clone(&state);
        let cancelled = tokio::spawn(async move { standing.stand_down(far_deadline()).await });
        while state.state_tasks.admits_new_work() {
            tokio::task::yield_now().await;
        }
        cancelled.abort();
        let _ = cancelled.await;

        let short = || std::time::Instant::now() + Duration::from_millis(30);
        let (first, second) = tokio::join!(state.stand_down(short()), state.stand_down(short()));
        for outcome in [first, second] {
            assert!(
                matches!(&outcome, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("held owned work"))),
                "owned work still running is reported by every call: {outcome:?}"
            );
        }
        release.notify_one();
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
    }

    /// A caller admitted before the close holds the state through its
    /// writes: the stand-down waits for it, and no new caller is admitted.
    #[tokio::test]
    async fn an_admitted_caller_holds_the_stand_down_until_it_is_done() {
        let state = standing_state();
        let mob_id = state
            .mob_create_definition(explicit_definition("inserted-later"))
            .await
            .expect("create a mob whose handle is inserted later");
        let handle = state.handle_for(&mob_id).await.expect("its handle");
        let admitted = state
            .state_tasks
            .admit("test create")
            .expect("an open state admits a caller");

        let short = || std::time::Instant::now() + Duration::from_millis(30);
        let early = state.close_admission(short()).await;
        assert!(
            matches!(&early, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("test create"))),
            "an admitted caller still writing is reported: {early:?}"
        );
        assert!(
            state.state_tasks.admit("late create").is_none(),
            "a closed state admits no new caller"
        );
        let second_half = state
            .state_tasks
            .hold("second half")
            .expect("a closed but unsealed state still admits the second half");
        drop(admitted);
        assert_eq!(
            state.close_admission(far_deadline()).await,
            MobStateStandDown::Clean,
            "admission settles once the admitted caller is done"
        );
        let held = state.stand_down(short()).await;
        assert!(
            matches!(&held, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("second half"))),
            "a second-half hold keeps the stand-down open: {held:?}"
        );
        drop(second_half);
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
        assert!(
            state.state_tasks.hold("after seal").is_none(),
            "a sealed state admits nothing"
        );
        let refused = state
            .try_mob_insert_handle(MobId::from("inserted-after-seal"), handle)
            .await
            .expect_err("a sealed state inserts no handle");
        assert!(refused.to_string().contains("stood down"), "{refused}");
    }

    /// Closing the state's admission also closes every managed mob's own
    /// admission (D67), so its actor refuses new work too.
    #[tokio::test]
    async fn closing_the_state_closes_each_managed_mob() {
        let state = standing_state();
        let mob_id = state
            .mob_create_definition(explicit_definition("closed-with-the-state"))
            .await
            .expect("create a managed mob");
        let handle = state.handle_for(&mob_id).await.expect("its handle");
        assert!(handle.admission_closed().borrow().is_none());
        assert_eq!(
            state.close_admission(far_deadline()).await,
            MobStateStandDown::Clean
        );
        assert_eq!(
            *handle.admission_closed().borrow(),
            Some(meerkat_mob::MobAdmissionCloseReason::StandDown)
        );
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
    }

    /// A member tools build binds only through the create that manages its
    /// mob. Two creates of one id both build (the second passes the
    /// existence check while the first waits at the insertion seam); only
    /// the one that enters the managed set runs its on-managed hook.
    #[tokio::test]
    async fn only_the_create_that_manages_its_mob_runs_its_member_tools_hook() {
        let built = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let managed = Arc::new(std::sync::Mutex::new(Vec::<MobId>::new()));
        let factory: MemberExternalToolsFactory = Arc::new({
            let built = Arc::clone(&built);
            let managed = Arc::clone(&managed);
            move |_mob_id: &MobId| {
                built.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let managed = Arc::clone(&managed);
                let provider: meerkat_mob::ExternalToolsProvider = Arc::new(|| None);
                Some(MemberExternalTools {
                    provider,
                    on_managed: Some(Box::new(move |mob_id: &MobId| {
                        managed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(mob_id.clone());
                    })),
                })
            }
        });
        let state = MobMcpState::new(
            Arc::new(MockSessionSvc::new()),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("runtime custody")
        .with_member_external_tools_factory(factory)
        .into_shared();
        let hold = Arc::new(InsertHold::default());
        *state
            .create_insert_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hold));
        let reached = |hold: Arc<InsertHold>| async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("a create reaches the insertion seam");
        };

        let first_state = Arc::clone(&state);
        let first = tokio::spawn(async move {
            first_state
                .mob_create_definition(explicit_definition("hooked-once"))
                .await
        });
        reached(Arc::clone(&hold)).await;
        hold.reached
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let second_state = Arc::clone(&state);
        let second = tokio::spawn(async move {
            second_state
                .mob_create_definition(explicit_definition("hooked-once"))
                .await
        });
        // The second create builds too: it either fails its build or waits
        // at the same seam.
        tokio::time::timeout(Duration::from_secs(10), async {
            while !second.is_finished() && !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second create builds");
        assert_eq!(
            built.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both creates built member tools"
        );
        assert!(
            managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "no hook runs before a mob is managed"
        );
        // Release every create parked at the seam; a permit stored before a
        // create registers is taken by it.
        tokio::time::timeout(Duration::from_secs(20), async {
            while !(first.is_finished() && second.is_finished()) {
                hold.release.notify_one();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both creates finish once released");
        let outcomes = [
            first.await.expect("first create task"),
            second.await.expect("second create task"),
        ];
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "exactly one create manages the mob: {outcomes:?}"
        );
        assert_eq!(
            *managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![MobId::from("hooked-once")],
            "only the managing create ran its hook"
        );
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
    }

    /// A create whose caller is cancelled after its native mob started and
    /// before the managed-set insertion still inserts that mob: the create
    /// is the state's own task, so no started mob escapes the managed set,
    /// and a stand-down waits for the insertion.
    #[tokio::test]
    async fn a_create_cancelled_at_insertion_still_inserts_its_mob() {
        let state = standing_state();
        let hold = Arc::new(InsertHold::default());
        *state
            .create_insert_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hold));
        let creating = Arc::clone(&state);
        let caller = tokio::spawn(async move {
            creating
                .mob_create_definition(explicit_definition("cancelled-at-insert"))
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the create reaches the insertion seam");
        caller.abort();
        let cancelled = caller.await.expect_err("the caller was cancelled");
        assert!(cancelled.is_cancelled());

        let early = state
            .stand_down(std::time::Instant::now() + Duration::from_millis(50))
            .await;
        assert!(
            matches!(&early, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("mob create"))),
            "the parked create holds the stand-down: {early:?}"
        );
        hold.release.notify_one();
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
        assert!(
            state
                .mobs
                .read()
                .await
                .contains_key(&MobId::from("cancelled-at-insert")),
            "the create finished its insertion without its caller"
        );
    }

    /// A close that cannot finish by its deadline (the managed set is held)
    /// reports what it missed instead of waiting; a later close finishes it.
    #[tokio::test]
    async fn a_close_that_misses_its_deadline_reports_it() {
        let state = standing_state();
        state
            .mob_create_definition(explicit_definition("close-after-held-map"))
            .await
            .expect("create a managed mob");
        let held = state.mobs.write().await;
        let missed = state
            .close_admission(std::time::Instant::now() + Duration::from_millis(50))
            .await;
        assert!(
            matches!(&missed, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("before the deadline"))),
            "a held managed set is reported, not waited on: {missed:?}"
        );
        drop(held);
        assert_eq!(
            state.close_admission(far_deadline()).await,
            MobStateStandDown::Clean
        );
    }

    /// Owned work that ends without finishing (it panicked) loses its
    /// outcome: no stand-down is clean after it.
    #[tokio::test]
    async fn owned_work_that_panicked_keeps_every_stand_down_unsettled() {
        let state = standing_state();
        let rx = state
            .state_tasks
            .spawn(
                state_tasks::StateTaskKind::Owned,
                "panicking owned",
                async {
                    panic!("owned work failed");
                },
            )
            .expect("owned work admitted");
        let _ = rx.await;
        for _ in 0..2 {
            let outcome = state.stand_down(far_deadline()).await;
            assert!(
                matches!(&outcome, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("panicking owned"))),
                "the lost outcome is reported by every call: {outcome:?}"
            );
        }
    }

    /// The state's own background loops (the local forked-participant
    /// sweeper, scheduled by restore) are tracked and stood down.
    #[tokio::test]
    async fn stand_down_stops_the_scheduled_sweeper() {
        let state = standing_state();
        state.set_local_forked_participant_sweep_interval(Duration::from_millis(1));
        state
            .mob_list()
            .await
            .expect("restore schedules the sweeper");
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
        // A sealed state schedules nothing again.
        state.mob_list().await.expect("listing still answers");
        assert_eq!(
            state.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
    }

    /// Seat a worker in a child mob owned by a persisted owner session, stop
    /// the mob, and cold-restart the service. Returns the restarted service,
    /// the mob, its owner and the worker's bound session.
    async fn seat_child_worker_then_exit(
        svc: &Arc<MockSessionSvc>,
        root: &std::path::Path,
        mob_name: &str,
    ) -> (Arc<MockSessionSvc>, MobId, SessionId, SessionId) {
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.to_path_buf())),
        );
        let owner = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner.clone()))
            .await;
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition(mob_name),
                owner.clone(),
                true,
                false,
            )
            .await
            .expect("create child mob");
        let worker = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                worker.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let session = state
            .mob_resolve_bridge_session_id(&mob_id, &worker)
            .await
            .expect("resolve bridge session")
            .expect("worker bridge session");
        state
            .handle_for(&mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
        (Arc::new(svc.cold_restart().await), mob_id, owner, session)
    }

    /// The per-mob store databases under a persistent root.
    fn mob_store_files(root: &std::path::Path) -> usize {
        std::fs::read_dir(root.join("mobs"))
            .expect("read the persistent root")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with(MobMcpState::MOB_STORE_FILE_PREFIX) && name.ends_with(".db")
                })
            })
            .count()
    }

    fn restored_state(svc: &Arc<MockSessionSvc>, root: &std::path::Path) -> Arc<MobMcpState> {
        Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.to_path_buf())),
        )
    }

    /// A lookup of a held child mob answers its hold: the runtime-unavailable
    /// class with the bounded `reason` token, never a missing mob.
    fn assert_held_before_activation(result: Result<MobHandle, MobError>, reason: &str) {
        let Err(error) = result else {
            panic!("a held child mob has no handle");
        };
        assert!(error.is_held_before_activation(), "{error}");
        assert!(!error.is_missing_target(), "{error}");
        assert_eq!(
            error.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionRuntimeUnavailable)
        );
        assert_eq!(
            error
                .structured_data()
                .and_then(|data| data
                    .get("reason")
                    .and_then(|value| value.as_str().map(str::to_string)))
                .as_deref(),
            Some(reason)
        );
    }

    /// The single per-mob store database under a persistent root.
    fn only_mob_store_file(root: &std::path::Path) -> std::path::PathBuf {
        let mut files = std::fs::read_dir(root.join("mobs"))
            .expect("read the persistent root")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(MobMcpState::MOB_STORE_FILE_PREFIX)
                            && name.ends_with(".db")
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1, "exactly one mob store");
        files.remove(0)
    }

    /// A child mob whose owner is committed restores and activates its members.
    #[tokio::test]
    async fn restored_child_mob_with_a_committed_owner_activates() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "owner-committed").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed { revision: None }),
        );
        let activations_before = svc.session_activation_count();
        let restored = restored_state(&svc, root.path());
        let status = restored
            .mob_member_status(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("restore member status");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
        assert!(
            svc.session_activation_count() > activations_before,
            "control: an activating restore rebuilds the member session"
        );
    }

    /// A child mob whose owner is confirmed archived is torn down at restore
    /// without activation: its member session is never rebuilt, its storage
    /// is removed, and it stays gone across another restart.
    #[tokio::test]
    async fn restored_child_mob_of_an_archived_owner_is_destroyed() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "owner-archived").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Archived),
        );
        let activations_before = svc.session_activation_count();
        let restored = restored_state(&svc, root.path());
        assert!(
            matches!(
                restored.handle_for(&mob_id).await,
                Err(MobError::MobNotFound(_))
            ),
            "the archived owner's child mob is destroyed"
        );
        assert!(
            mob_store_files(root.path()) == 0,
            "the child mob's storage is removed"
        );
        assert_eq!(
            svc.session_activation_count(),
            activations_before,
            "the teardown never activates the member"
        );
        let again = restored_state(&Arc::new(svc.cold_restart().await), root.path());
        assert!(matches!(
            again.handle_for(&mob_id).await,
            Err(MobError::MobNotFound(_))
        ));
    }

    /// A child mob whose owner has no durable authority (`Absent`) is held:
    /// never resumed, reported as unresolved rather than gone, its storage
    /// kept, while the rest of the restore succeeds. Once the owner is
    /// committed, a scavenger pass resumes it.
    #[tokio::test]
    async fn restored_child_mob_of_an_absent_owner_is_held_until_resolved() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "owner-absent").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let restored = restored_state(&svc, root.path());
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");
        assert!(
            restored.mob_list().await.is_ok(),
            "a held child does not fail the restore"
        );
        assert_eq!(restored.held_child_mobs().await.len(), 1);
        assert_eq!(
            mob_store_files(root.path()),
            1,
            "the held child's storage remains"
        );

        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed { revision: None }),
        );
        assert!(
            restored
                .scavenge_orphaned_bridge_session_scoped_mobs()
                .await
                .is_empty(),
            "a committed owner destroys nothing"
        );
        assert!(restored.held_child_mobs().await.is_empty());
        let status = restored
            .mob_member_status(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("the resolved child resumes");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
    }

    /// A store fault while observing the owner holds the child like an
    /// absent owner: a fault is never evidence of an archive.
    #[tokio::test]
    async fn restored_child_mob_is_held_when_its_owner_cannot_be_observed() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "owner-unreadable").await;
        svc.script_observation(&owner, Err("owner authority unreadable".to_string()));
        let restored = restored_state(&svc, root.path());
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_unobservable");
        assert_eq!(
            mob_store_files(root.path()),
            1,
            "the held child's storage remains"
        );
    }

    /// A create by a member's session records that member as its creator,
    /// with its mob and that mob's incarnation (`MobProvenance::Member`). The
    /// record reads back unchanged from the child's store while the child is
    /// held, and through its handle once it resumes.
    #[tokio::test]
    async fn a_member_session_create_records_member_provenance() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = restored_state(&svc, root.path());
        let parent = state
            .mob_create_definition(explicit_definition("provenance-parent"))
            .await
            .expect("create the parent mob");
        let worker = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &parent,
                ProfileName::from("worker"),
                worker.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn the creator");
        let creator_session = state
            .mob_resolve_bridge_session_id(&parent, &worker)
            .await
            .expect("resolve bridge session")
            .expect("creator bridge session");
        let parent_handle = state.handle_for(&parent).await.expect("parent handle");
        let (identity, generation) = parent_handle
            .member_of_session(&creator_session)
            .await
            .expect("the parent roster names the creator's session");
        assert_eq!(identity, worker);
        let child = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("provenance-child"),
                creator_session.clone(),
                true,
                false,
            )
            .await
            .expect("the creator's session creates a child mob");
        let expected = meerkat_mob::MobProvenance::Member {
            parent_mob_id: parent.clone(),
            parent_incarnation: parent_handle.incarnation(),
            creator_identity: worker,
            creator_generation: generation,
            owner_session: creator_session.clone(),
        };
        assert_eq!(
            state
                .handle_for(&child)
                .await
                .expect("child handle")
                .provenance()
                .await
                .expect("provenance"),
            expected
        );
        for mob_id in [&parent, &child] {
            state
                .handle_for(mob_id)
                .await
                .expect("mob handle")
                .shutdown()
                .await
                .expect("stop the mob");
        }

        // After a restart the child's owner (the creator's session) does not
        // resolve: the child is held, and its record reads from its store.
        let svc = Arc::new(svc.cold_restart().await);
        svc.script_observation(
            &creator_session,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let restored = restored_state(&svc, root.path());
        assert_eq!(restored.held_child_mobs().await.len(), 1);
        let held_path = restored
            .held_child_mobs
            .read()
            .await
            .get(&child)
            .map(|held| held.storage_path.clone())
            .expect("the child is held");
        let held_store = MobStorage::persistent(&held_path).expect("open the held store");
        assert_eq!(
            held_store
                .creation_provenance()
                .await
                .expect("read the held record"),
            Some(expected.clone())
        );
        drop(held_store);

        // Once the owner resolves, the child resumes with the same record.
        svc.script_observation(
            &creator_session,
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed { revision: None }),
        );
        assert!(
            restored
                .scavenge_orphaned_bridge_session_scoped_mobs()
                .await
                .is_empty()
        );
        assert_eq!(
            restored
                .handle_for(&child)
                .await
                .expect("the resumed child")
                .provenance()
                .await
                .expect("provenance"),
            expected
        );
    }

    /// A create with no caller session is the host's: it records `Host`,
    /// whatever the hosted rosters hold.
    #[tokio::test]
    async fn a_host_create_records_host_provenance() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let mob_id = state
            .mob_create_definition(explicit_definition("provenance-host"))
            .await
            .expect("host create");
        assert_eq!(
            state
                .handle_for(&mob_id)
                .await
                .expect("handle")
                .provenance()
                .await
                .expect("provenance"),
            meerkat_mob::MobProvenance::Host
        );
    }

    /// A create by a session that no hosted roster names records that
    /// session (`MobProvenance::Session`): the lookup was complete, so the
    /// fact is positive, never `Host`.
    #[tokio::test]
    async fn a_session_create_outside_every_roster_records_the_session() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let other = state
            .mob_create_definition(explicit_definition("provenance-other"))
            .await
            .expect("another hosted mob");
        state
            .mob_spawn(
                &other,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("a member whose session is not the caller");
        let caller = SessionId::new();
        svc.insert_persisted_session(Session::with_id(caller.clone()))
            .await;
        assert!(
            state
                .session_member_matches(&caller)
                .await
                .expect("complete lookup")
                .is_empty()
        );
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("provenance-session"),
                caller.clone(),
                false,
                false,
            )
            .await
            .expect("session create");
        assert_eq!(
            state
                .handle_for(&mob_id)
                .await
                .expect("handle")
                .provenance()
                .await
                .expect("provenance"),
            meerkat_mob::MobProvenance::Session { session_id: caller }
        );
    }

    /// Two hosted rosters naming the caller session record `Ambiguous` with
    /// their count, and never pick one of them; one names the member, none
    /// names the session.
    #[test]
    fn the_creation_provenance_never_picks_between_rosters() {
        let session_id = SessionId::new();
        let matched = |mob: &str| SessionMemberMatch {
            mob_id: MobId::from(mob),
            incarnation: meerkat_core::service::MobIncarnationId::mint(),
            identity: AgentIdentity::from("worker-1"),
            generation: meerkat_mob::Generation::INITIAL,
        };
        for caller in [SessionCreateCaller::Host, SessionCreateCaller::Session] {
            assert_eq!(
                session_creation_provenance(&session_id, vec![matched("a"), matched("b")], caller),
                meerkat_mob::MobProvenance::Ambiguous {
                    matching_rosters: 2
                }
            );
        }
        let one = matched("a");
        assert_eq!(
            session_creation_provenance(&session_id, vec![one.clone()], SessionCreateCaller::Host),
            meerkat_mob::MobProvenance::Member {
                parent_mob_id: one.mob_id.clone(),
                parent_incarnation: one.incarnation,
                creator_identity: one.identity.clone(),
                creator_generation: one.generation,
                owner_session: session_id.clone(),
            }
        );
        assert_eq!(
            session_creation_provenance(
                &session_id,
                vec![one.clone()],
                SessionCreateCaller::Session
            ),
            meerkat_mob::MobProvenance::OperatorMember {
                parent_mob_id: one.mob_id,
                parent_incarnation: one.incarnation,
                creator_identity: one.identity,
                creator_generation: one.generation,
                owner_session: session_id.clone(),
            }
        );
        assert_eq!(
            session_creation_provenance(&session_id, Vec::new(), SessionCreateCaller::Host),
            meerkat_mob::MobProvenance::Session {
                session_id: session_id.clone()
            }
        );
        assert_eq!(
            session_creation_provenance(&session_id, Vec::new(), SessionCreateCaller::Session),
            meerkat_mob::MobProvenance::OperatorSession {
                session_id: session_id.clone()
            }
        );
    }

    /// Every managed-set entry is built by the one helper that sets the
    /// host-insert latch, and only `try_mob_insert_handle` inserts as the
    /// host. An insert path that builds its entry itself, or a second
    /// host-insert site, fails here, so a host insert can never skip the
    /// latch that keeps the member lookup's second completeness arm honest.
    #[test]
    fn every_managed_set_entry_is_built_by_the_latching_helper() {
        // The shipped code of every source file of this crate: the code
        // before its `mod tests`, and no test-only module file.
        fn shipped_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("read the crate's sources") {
                let path = entry.expect("a source entry").path();
                if path.is_dir() {
                    shipped_sources(&path, out);
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read a source file");
                let shipped = source.split("\nmod tests {").next().unwrap_or_default();
                out.push((path, shipped.to_string()));
            }
        }
        // Where the method that starts with `signature` lies in `source`: up
        // to the end of its body at method indentation.
        fn method(source: &str, signature: &str) -> std::ops::Range<usize> {
            let start = source.find(signature).expect("the method exists");
            let end = source[start..]
                .find("\n    }\n")
                .map(|end| start + end)
                .expect("the method ends");
            start..end
        }
        let mut sources = Vec::new();
        shipped_sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut sources,
        );
        let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
        let lib_source = &sources
            .iter()
            .find(|(path, _)| path == &lib)
            .expect("the crate root is shipped code")
            .1;
        let helper = method(lib_source, "fn managed_mob_entry(");
        let host_insert = method(lib_source, "pub async fn try_mob_insert_handle(");
        let mut literals = Vec::new();
        let mut host_origins = Vec::new();
        for (path, source) in &sources {
            for (at, _) in source.match_indices("ManagedMob {") {
                // Neither the struct's own definition nor a function's return
                // type is an entry.
                if !source[..at].ends_with("struct ") && !source[..at].ends_with("-> ") {
                    literals.push((path.clone(), at));
                }
            }
            for (at, _) in source.match_indices("ManagedInsertOrigin::HostInsert") {
                if !(path == &lib && helper.contains(&at)) {
                    host_origins.push((path.clone(), at));
                }
            }
        }
        assert_eq!(
            literals.len(),
            1,
            "only the helper builds a managed-set entry: {literals:?}"
        );
        assert!(
            literals[0].0 == lib && helper.contains(&literals[0].1),
            "the one entry literal is the helper's: {literals:?}"
        );
        assert_eq!(
            host_origins.len(),
            1,
            "one host-insert site outside the helper: {host_origins:?}"
        );
        assert!(
            host_origins[0].0 == lib && host_insert.contains(&host_origins[0].1),
            "the host-insert site is try_mob_insert_handle: {host_origins:?}"
        );
    }

    /// A held child's store is read once per hold: after the first lookup
    /// read it, a damaged store changes no answer, for the session its
    /// roster names or for any other. (A read that fails is not kept: see
    /// `an_incomplete_lookup_refuses_the_create_and_names_the_blocking_mob`,
    /// whose first read happens after the damage.)
    #[tokio::test]
    async fn a_held_childs_store_is_read_once_per_hold() {
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, child, owner, worker_session) = seat_child_worker_then_exit(
            &Arc::new(MockSessionSvc::new()),
            root.path(),
            "held-read-once",
        )
        .await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let restored = restored_state(&svc, root.path());
        assert_eq!(restored.held_child_mobs().await.len(), 1);
        let first = restored
            .session_member_matches(&worker_session)
            .await
            .expect("the held child's roster is read");
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].mob_id, child);
        assert_eq!(first[0].identity, AgentIdentity::from("worker-1"));

        let held_path = restored
            .held_child_mobs
            .read()
            .await
            .get(&child)
            .map(|held| held.storage_path.clone())
            .expect("the child is held");
        std::fs::write(&held_path, b"not a mob store").expect("damage the held store");
        assert_eq!(
            restored
                .session_member_matches(&worker_session)
                .await
                .expect("the kept read answers"),
            first
        );
        assert!(
            restored
                .session_member_matches(&SessionId::new())
                .await
                .expect("the kept read answers for any session")
                .is_empty()
        );
    }

    /// The creator a mob's creation record proves, whose manage grant the
    /// agent mob tools re-derive after a restart: only a session's own create
    /// with its create capability. An OperatorSession record proves the
    /// creating session. An OperatorMember record proves the creating
    /// member's session only while that session still serves the recorded
    /// member at the recorded generation in the recorded parent incarnation:
    /// once the member is respawned at its next generation, neither its old
    /// session nor its new one proves anything. A create the host made for
    /// the same session or member (`Session`, `Member`) proves nothing: the
    /// session owns that mob but did not create it. A host create, and any
    /// other session, prove nothing.
    #[tokio::test]
    async fn a_creation_record_proves_its_creator_session_at_the_recorded_generation() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = restored_state(&svc, root.path());
        let parent = state
            .mob_create_definition(explicit_definition("creator-parent"))
            .await
            .expect("create the parent mob");
        let worker = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &parent,
                ProfileName::from("worker"),
                worker.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn the creator");
        let creator_session = state
            .mob_resolve_bridge_session_id(&parent, &worker)
            .await
            .expect("resolve bridge session")
            .expect("creator bridge session");
        let member_child = state
            .create_mob_for_owner_bridge_session(
                explicit_definition("creator-member-child"),
                creator_session.clone(),
                false,
                false,
                SessionCreateCaller::Session,
            )
            .await
            .expect("the member's session creates a mob")
            .mob_id;
        let member_child = state
            .handle_for(&member_child)
            .await
            .expect("member child handle");
        assert!(matches!(
            member_child.provenance().await.expect("provenance"),
            meerkat_mob::MobProvenance::OperatorMember { .. }
        ));
        assert!(
            state
                .session_is_recorded_creator(&member_child, &creator_session)
                .await
        );
        assert!(
            !state
                .session_is_recorded_creator(&member_child, &SessionId::new())
                .await
        );
        let member_owned = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("creator-member-owned"),
                creator_session.clone(),
                false,
                false,
            )
            .await
            .expect("the host creates a mob for the member's session");
        let member_owned = state
            .handle_for(&member_owned)
            .await
            .expect("member owned handle");
        assert!(matches!(
            member_owned.provenance().await.expect("provenance"),
            meerkat_mob::MobProvenance::Member { .. }
        ));
        assert!(
            !state
                .session_is_recorded_creator(&member_owned, &creator_session)
                .await,
            "a mob the host created for the member's session is not the member's create"
        );

        let caller = SessionId::new();
        svc.insert_persisted_session(Session::with_id(caller.clone()))
            .await;
        let session_child = state
            .create_mob_for_owner_bridge_session(
                explicit_definition("creator-session-child"),
                caller.clone(),
                false,
                false,
                SessionCreateCaller::Session,
            )
            .await
            .expect("a session creates a mob")
            .mob_id;
        let session_child = state
            .handle_for(&session_child)
            .await
            .expect("session child handle");
        assert_eq!(
            session_child.provenance().await.expect("provenance"),
            meerkat_mob::MobProvenance::OperatorSession {
                session_id: caller.clone()
            }
        );
        assert!(
            state
                .session_is_recorded_creator(&session_child, &caller)
                .await
        );
        assert!(
            !state
                .session_is_recorded_creator(&session_child, &creator_session)
                .await
        );
        let session_owned = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("creator-session-owned"),
                caller.clone(),
                true,
                false,
            )
            .await
            .expect("the host creates a mob for the session");
        let session_owned = state
            .handle_for(&session_owned)
            .await
            .expect("session owned handle");
        assert!(
            !state
                .session_is_recorded_creator(&session_owned, &caller)
                .await,
            "a mob the host created for the session is not the session's create"
        );

        let host = state
            .mob_create_definition(explicit_definition("creator-host"))
            .await
            .expect("a host create");
        let host = state.handle_for(&host).await.expect("host handle");
        assert!(!state.session_is_recorded_creator(&host, &caller).await);
        assert!(
            !state
                .session_is_recorded_creator(&host, &creator_session)
                .await
        );

        state
            .mob_respawn(&parent, worker.clone(), None)
            .await
            .expect("respawn the creating member at its next generation");
        let respawned_session = state
            .mob_resolve_bridge_session_id(&parent, &worker)
            .await
            .expect("resolve the respawned bridge session")
            .expect("respawned bridge session");
        assert!(
            !state
                .session_is_recorded_creator(&member_child, &creator_session)
                .await,
            "the old session no longer serves the recorded generation"
        );
        assert!(
            !state
                .session_is_recorded_creator(&member_child, &respawned_session)
                .await,
            "a later generation of the creating member proves nothing"
        );
    }

    /// A session create whose lookup cannot be complete is refused before
    /// anything of the mob exists: while a state that inserted handles has
    /// not declared its mob set restored (no mob named), and while a held
    /// child's roster cannot be read (that child named). A host create runs
    /// no lookup and is not refused.
    #[tokio::test]
    async fn an_incomplete_lookup_refuses_the_create_and_names_the_blocking_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let host = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let inserted = host
            .mob_create_definition(explicit_definition("provenance-inserted"))
            .await
            .expect("a mob the host restores by insertion");
        state
            .try_mob_insert_handle(
                inserted.clone(),
                host.handle_for(&inserted).await.expect("host handle"),
            )
            .await
            .expect("insert the host's mob");
        let caller = SessionId::new();
        svc.insert_persisted_session(Session::with_id(caller.clone()))
            .await;
        let refused = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("provenance-refused"),
                caller.clone(),
                false,
                false,
            )
            .await
            .expect_err("an undeclared restore leaves the lookup incomplete");
        assert!(
            matches!(refused, MobError::ProvenanceUnavailable { mob_id: None }),
            "{refused:?}"
        );
        assert!(
            state
                .handle_for(&MobId::from("provenance-refused"))
                .await
                .is_err(),
            "nothing of the refused mob exists"
        );
        state
            .mob_create_definition(explicit_definition("provenance-host-anyway"))
            .await
            .expect("a host create runs no lookup");
        state.declare_mob_set_restored();
        state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("provenance-refused"),
                caller,
                false,
                false,
            )
            .await
            .expect("the declared set completes the lookup");

        // A held child whose store cannot be read blocks every session
        // create, named.
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, child, owner, _worker_session) = seat_child_worker_then_exit(
            &Arc::new(MockSessionSvc::new()),
            root.path(),
            "held-unreadable",
        )
        .await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let restored = restored_state(&svc, root.path());
        assert_eq!(restored.held_child_mobs().await.len(), 1);
        let held_path = restored
            .held_child_mobs
            .read()
            .await
            .get(&child)
            .map(|held| held.storage_path.clone())
            .expect("the child is held");
        std::fs::write(&held_path, b"not a mob store").expect("damage the held store");
        let caller = SessionId::new();
        svc.insert_persisted_session(Session::with_id(caller.clone()))
            .await;
        let refused = restored
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("provenance-blocked"),
                caller,
                false,
                false,
            )
            .await
            .expect_err("an unreadable held roster leaves the lookup incomplete");
        assert!(
            matches!(&refused, MobError::ProvenanceUnavailable { mob_id: Some(blocking) } if blocking == &child),
            "{refused:?}"
        );
        restored
            .mob_create_definition(explicit_definition("provenance-host-held"))
            .await
            .expect("a host create is not blocked");
    }

    /// A handle a host inserts while a member lookup scans may hold a roster
    /// the scan did not read: the lookup reads its completeness again after
    /// the scan, and the create is refused even though the inserted roster
    /// names the caller session.
    #[tokio::test]
    async fn a_handle_inserted_during_the_lookup_refuses_the_create() {
        let svc = Arc::new(MockSessionSvc::new());
        let host = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let inserted = host
            .mob_create_definition(explicit_definition("provenance-racing-insert"))
            .await
            .expect("the mob the host inserts");
        let worker = AgentIdentity::from("worker-1");
        host.mob_spawn(
            &inserted,
            ProfileName::from("worker"),
            worker.clone(),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("spawn the member whose session creates");
        let caller = host
            .mob_resolve_bridge_session_id(&inserted, &worker)
            .await
            .expect("resolve bridge session")
            .expect("member bridge session");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody"),
        );
        let hold = Arc::new(InsertHold::default());
        *state
            .member_lookup_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hold));
        let creating = Arc::clone(&state);
        let create_caller = caller.clone();
        let create = tokio::spawn(async move {
            creating
                .mob_create_definition_with_owner_bridge_session(
                    explicit_definition("provenance-during-insert"),
                    create_caller,
                    false,
                    false,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the lookup passed its first completeness check");
        state
            .try_mob_insert_handle(
                inserted.clone(),
                host.handle_for(&inserted).await.expect("host handle"),
            )
            .await
            .expect("insert the host's mob while the lookup is parked");
        hold.release.notify_one();
        let refused = create
            .await
            .expect("the create task ends")
            .expect_err("an insert during the lookup leaves it incomplete");
        assert!(
            matches!(refused, MobError::ProvenanceUnavailable { mob_id: None }),
            "{refused:?}"
        );
        assert!(
            state
                .handle_for(&MobId::from("provenance-during-insert"))
                .await
                .is_err(),
            "nothing of the refused mob exists"
        );
    }

    /// An operator releases a held child whose owner never resolves: it is
    /// destroyed through the ordinary destroy envelope.
    #[tokio::test]
    async fn operator_destroys_a_held_child_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "owner-never-resolves").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let activations_before = svc.session_activation_count();
        let restored = restored_state(&svc, root.path());
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");

        // A release cancelled while it holds at its WorkGraph preflight
        // leaves the child held: typed lookup, storage kept, nothing
        // activated. (This state is not shared, so the release runs on its
        // caller; the shared, state-owned handoff is pinned below.)
        let hold = hold_teardown_preflight(&restored);
        cancel_at_teardown_preflight(restored.destroy_held_child_mob(&mob_id), &hold).await;
        release_teardown_preflight(&restored);
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");
        assert_eq!(
            mob_store_files(root.path()),
            1,
            "a cancelled release keeps storage"
        );
        assert_eq!(svc.session_activation_count(), activations_before);

        restored
            .destroy_held_child_mob(&mob_id)
            .await
            .expect("the operator releases the held child");
        assert!(matches!(
            restored.handle_for(&mob_id).await,
            Err(MobError::MobNotFound(_))
        ));
        assert!(restored.held_child_mobs().await.is_empty());
        assert_eq!(
            mob_store_files(root.path()),
            0,
            "the released child's storage is removed"
        );
        assert_eq!(
            svc.session_activation_count(),
            activations_before,
            "the operator release never activates the member"
        );
    }

    /// On a shared state the held child's release hands over to a task the
    /// state owns: a caller cancelled at the WorkGraph preflight leaves the
    /// handoff parked and the stand-down unsettled; once the gate is
    /// notified, the handoff finishes without its caller and the stand-down
    /// settles.
    #[tokio::test]
    async fn a_cancelled_shared_release_finishes_as_a_state_task() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "shared-release").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let activations_before = svc.session_activation_count();
        let restored = MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
            .expect("runtime custody")
            .with_persistent_storage_root(Some(root.path().to_path_buf()))
            .into_shared();
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");

        let hold = hold_teardown_preflight(&restored);
        cancel_at_teardown_preflight(restored.destroy_held_child_mob(&mob_id), &hold).await;
        let parked = restored
            .stand_down(std::time::Instant::now() + Duration::from_millis(50))
            .await;
        assert!(
            matches!(&parked, MobStateStandDown::Unsettled { reasons } if reasons.iter().any(|reason| reason.contains("child mob teardown handoff"))),
            "the parked handoff holds the stand-down: {parked:?}"
        );
        assert_eq!(
            mob_store_files(root.path()),
            1,
            "nothing is torn down while the handoff is parked"
        );

        // Clearing the gate's slot would not wake the parked task: notify it.
        hold.release.notify_one();
        assert_eq!(
            restored.stand_down(far_deadline()).await,
            MobStateStandDown::Clean
        );
        assert!(
            restored.held_child_mobs().await.is_empty(),
            "the handoff moved the child into managed custody without its caller"
        );
        assert_eq!(svc.session_activation_count(), activations_before);
    }

    /// A scavenger pass cancelled at its teardown preflight, with the owner
    /// now archived, leaves the child held; a later pass tears it down.
    #[tokio::test]
    async fn cancelled_reobservation_keeps_the_child_held() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "reobserve-cancelled").await;
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let activations_before = svc.session_activation_count();
        let restored = restored_state(&svc, root.path());
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");

        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Archived),
        );
        let hold = hold_teardown_preflight(&restored);
        cancel_at_teardown_preflight(
            restored.scavenge_orphaned_bridge_session_scoped_mobs(),
            &hold,
        )
        .await;
        release_teardown_preflight(&restored);
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");
        assert_eq!(
            mob_store_files(root.path()),
            1,
            "a cancelled pass keeps storage"
        );
        assert_eq!(svc.session_activation_count(), activations_before);

        assert_eq!(
            restored
                .scavenge_orphaned_bridge_session_scoped_mobs()
                .await,
            vec![mob_id.clone()],
            "a later pass tears the child down"
        );
        assert!(restored.held_child_mobs().await.is_empty());
        assert_eq!(mob_store_files(root.path()), 0);
        assert_eq!(svc.session_activation_count(), activations_before);
    }

    /// Park every child teardown of `state` right before its WorkGraph
    /// preflight.
    fn hold_teardown_preflight(state: &MobMcpState) -> Arc<TeardownPreflightHold> {
        let hold = Arc::new(TeardownPreflightHold::default());
        *state
            .teardown_preflight_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hold.clone());
        hold
    }

    fn release_teardown_preflight(state: &MobMcpState) {
        *state
            .teardown_preflight_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Drive `future` until its teardown parks at the preflight, then drop
    /// it: the caller is cancelled exactly there.
    async fn cancel_at_teardown_preflight<F: std::future::Future>(
        future: F,
        hold: &TeardownPreflightHold,
    ) {
        let mut future = std::pin::pin!(future);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the teardown reaches its preflight"
            );
            tokio::select! {
                biased;
                _ = &mut future => panic!("the transfer completed before its preflight"),
                () = tokio::task::yield_now() => {}
            }
        }
    }

    /// The owner binding survives a reset, as cold resume recovers it: a
    /// child reset before restart is still held when its owner is absent.
    #[tokio::test]
    async fn restored_child_mob_keeps_its_owner_binding_across_a_reset() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = restored_state(&svc, root.path());
        let owner = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner.clone()))
            .await;
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("owner-bound-then-reset"),
                owner.clone(),
                true,
                false,
            )
            .await
            .expect("create child mob");
        state.mob_reset(&mob_id).await.expect("reset the child mob");
        state
            .handle_for(&mob_id)
            .await
            .expect("child mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
        let svc = Arc::new(svc.cold_restart().await);
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Absent),
        );
        let restored = restored_state(&svc, root.path());
        assert_held_before_activation(restored.handle_for(&mob_id).await, "owner_absent");
    }

    /// A durable destroy admission is recovered whatever the owner reads as:
    /// with the owner unobservable, the child is not held; it resumes into
    /// the recovery of its admitted destroy.
    #[tokio::test]
    async fn admitted_destroy_recovers_when_the_owner_cannot_be_observed() {
        use meerkat_mob::MobEventStore as _;
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "admitted-then-unobservable").await;
        meerkat_mob::SqliteMobStores::open(only_mob_store_file(root.path()))
            .expect("open the child store")
            .event_store()
            .append(meerkat_mob::NewMobEvent::for_test(
                mob_id.clone(),
                meerkat_mob::MobEventKind::MobDestroying,
            ))
            .await
            .expect("record an admitted destroy");
        svc.script_observation(&owner, Err("owner authority unreadable".to_string()));
        let restored = restored_state(&svc, root.path());
        let handle = restored
            .handle_for(&mob_id)
            .await
            .expect("an admitted destroy resumes into its recovery, not a hold");
        assert!(restored.held_child_mobs().await.is_empty());
        assert_eq!(
            handle.status().await.expect("recovered status"),
            meerkat_mob::MobState::Destroyed,
            "the recovered child's public authority stays closed"
        );
    }

    /// A durable Destroy quiesce without `MobDestroying` (a destroy that
    /// latched but did not finish its admission) is read from history too: it
    /// resumes into the existing recovered intent, not a hold or a fresh
    /// admission, even with the owner unobservable.
    #[tokio::test]
    async fn durable_destroy_quiesce_recovers_when_the_owner_cannot_be_observed() {
        use meerkat_mob::MobEventStore as _;
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, owner, _worker_session) =
            seat_child_worker_then_exit(&svc, root.path(), "quiesced-then-unobservable").await;
        meerkat_mob::SqliteMobStores::open(only_mob_store_file(root.path()))
            .expect("open the child store")
            .event_store()
            .append(meerkat_mob::NewMobEvent::for_test(
                mob_id.clone(),
                meerkat_mob::MobEventKind::PlacedCompletionLifecycleQuiesceStarted {
                    intent:
                        meerkat_mob::machines::mob_machine::PlacedCompletionLifecycleIntentKind::Destroy,
                },
            ))
            .await
            .expect("record a durable Destroy quiesce");
        svc.script_observation(&owner, Err("owner authority unreadable".to_string()));
        let restored = restored_state(&svc, root.path());
        restored
            .handle_for(&mob_id)
            .await
            .expect("a durable Destroy quiesce resumes into its recovery, not a hold");
        assert!(restored.held_child_mobs().await.is_empty());
    }

    /// RED control for the old scavenger: an owner that merely has no session
    /// (read reports `NotFound`, the observation reports `Absent`) is not an
    /// archived owner. The child mob and an unrelated host mob both stay.
    #[tokio::test]
    async fn scavenger_keeps_a_child_whose_owner_session_is_not_found() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let owner = SessionId::new();
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("owner-never-persisted"),
                owner.clone(),
                true,
                false,
            )
            .await
            .expect("create child mob");
        assert!(
            matches!(svc.read(&owner).await, Err(SessionError::NotFound { .. })),
            "the owner session reads as not found"
        );
        let host_mob = state
            .mob_create_definition(explicit_definition("owner-never-persisted-sibling"))
            .await
            .expect("create an unrelated host mob");
        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert!(
            scavenged.is_empty(),
            "a not-found owner is not a confirmed archive"
        );
        assert!(
            state.handle_for(&mob_id).await.is_ok(),
            "the child mob is not destroyed"
        );
        assert!(
            state.handle_for(&host_mob).await.is_ok(),
            "the host mob is unaffected"
        );
    }

    /// #1701: restoring a child mob asks the host customizer for each member's
    /// rebuild through `customize_resume`, with the member's bound session.
    #[tokio::test]
    async fn restored_child_mob_members_are_customized_with_their_resume_view() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, session) =
            seat_worker_then_exit(&svc, root.path(), true, "restored-customized-child").await;
        let host = Arc::new(WideningHostCustomizer::default());
        let restored = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.path().to_path_buf()))
                .with_child_spawn_member_customizer(host.clone()),
        );
        let status = restored
            .mob_member_status(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("restore member status");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
        let calls = host.calls();
        assert!(
            calls
                .iter()
                .any(|(mob, source, member, view)| mob == &mob_id
                    && *source == meerkat_mob::SpawnSource::Resume
                    && member.as_str() == "worker-1"
                    && view.as_ref() == Some(&session)),
            "the restore asks the host customizer with the worker's session: {calls:?}"
        );
    }

    /// #1701 restore ordering, under the joint child-policy decision: a
    /// restored mob is classified as a child mob before any member is
    /// restored, so its members' rebuilds run under the child policy. A
    /// managed host without a child policy still restores everything: the
    /// child mob comes up, and its members are not brought back unconstrained.
    /// Each carries a typed restore failure saying why and naming the fixes,
    /// and calls into it return that refusal. (Recovery once the refusal
    /// lifts is proved in meerkat-mob:
    /// `refused_restore_is_per_member_and_recovers_once_admitted`.)
    #[tokio::test]
    async fn unconfigured_child_policy_refuses_restored_child_members_not_the_restore() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let worker = AgentIdentity::from("worker-1");
        let seating = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let owner = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner.clone()))
            .await;
        let child = seating
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("restored-policy-child"),
                owner,
                true,
                false,
            )
            .await
            .expect("create child mob");
        let host_mob = seating
            .mob_create_definition(explicit_definition("restored-policy-host"))
            .await
            .expect("create host mob");
        for mob_id in [&child, &host_mob] {
            seating
                .mob_spawn(
                    mob_id,
                    ProfileName::from("worker"),
                    worker.clone(),
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                    None,
                )
                .await
                .expect("spawn worker");
            seating
                .handle_for(mob_id)
                .await
                .expect("predecessor mob handle")
                .shutdown()
                .await
                .expect("retire the predecessor supervisor route");
        }
        let svc = Arc::new(svc.cold_restart().await);

        let unconfigured = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.path().to_path_buf()))
                .with_tool_consequence_policy_registry(managed_policy_registry()),
        );
        let mobs = unconfigured
            .mob_list()
            .await
            .expect("the restore completes");
        for mob_id in [&child, &host_mob] {
            assert!(
                mobs.iter().any(|(id, _)| id == mob_id),
                "{mob_id} is restored: {mobs:?}"
            );
        }
        assert_eq!(
            unconfigured
                .mob_member_status(&host_mob, &worker)
                .await
                .expect("host mob member status")
                .status,
            meerkat_mob::MobMemberStatus::Active,
            "the host mob's member restores"
        );
        let refusal = unconfigured
            .handle_for(&child)
            .await
            .expect("the child mob is restored")
            .member(&worker)
            .await
            .err()
            .expect("the child member is refused");
        let MobError::MemberRestoreFailed { reason, .. } = &refusal else {
            panic!("a typed restore failure, got {refusal:?}");
        };
        for needle in [
            "no child application tool policy is configured",
            "MobMcpState::with_child_application_tool_policy(binding)",
            "child_application_tool_policy",
            "ApplicationToolPolicyBinding::Unmanaged",
        ] {
            assert!(reason.contains(needle), "{needle} missing from: {reason}");
        }
    }

    /// The in-memory service shared by every mob of one state reports each
    /// live session as belonging only to the mob it was built for, so the
    /// aggregate owner check (owner routing, archive classification) never
    /// reports another mob's member, or a live session no mob built, as
    /// mob-owned on that mob's account.
    #[tokio::test]
    async fn in_memory_sessions_belong_only_to_their_own_mob() {
        let state = MobMcpState::new_in_memory();
        let first = state
            .mob_create_definition(explicit_definition("ownership-first"))
            .await
            .expect("create first mob");
        let second = state
            .mob_create_definition(explicit_definition("ownership-second"))
            .await
            .expect("create second mob");
        let worker = AgentIdentity::from("worker-1");
        for mob_id in [&first, &second] {
            state
                .mob_spawn(
                    mob_id,
                    ProfileName::from("worker"),
                    worker.clone(),
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                    None,
                )
                .await
                .expect("spawn worker");
        }
        let first_session = state
            .mob_resolve_bridge_session_id(&first, &worker)
            .await
            .expect("resolve first session")
            .expect("first worker session");
        let second_session = state
            .mob_resolve_bridge_session_id(&second, &worker)
            .await
            .expect("resolve second session")
            .expect("second worker session");
        let service = state.session_service();
        assert!(service.session_belongs_to_mob(&first_session, &first).await);
        assert!(
            !service
                .session_belongs_to_mob(&first_session, &second)
                .await
        );
        assert!(
            service
                .session_belongs_to_mob(&second_session, &second)
                .await
        );
        assert!(
            !service
                .session_belongs_to_mob(&second_session, &first)
                .await
        );
        assert!(
            !service
                .session_belongs_to_mob(&SessionId::new(), &first)
                .await,
            "an unknown session is no mob's"
        );
        assert!(
            state
                .owns_service_reported_bridge_session(&first_session)
                .await
        );

        // A live session no mob built is not mob-owned, although mobs exist.
        let unbound = Session::new();
        let unbound_id = unbound.id().clone();
        service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-opus-5".into(),
                prompt: "unbound".into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    resume_session: Some(unbound),
                    ..Default::default()
                }),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .expect("create a live session no mob built");
        assert!(service.has_live_session(&unbound_id).await.expect("live"));
        assert!(!service.session_belongs_to_mob(&unbound_id, &first).await);
        assert!(
            !state
                .owns_service_reported_bridge_session(&unbound_id)
                .await,
            "a live session no mob built is not reported as mob-owned"
        );
    }

    /// #1701 control: the managed host without a child policy that refuses a
    /// restored child mob's members (above) restores a host-created mob's
    /// members as before: it is never classified as a child mob, so the child
    /// policy does not govern its members' rebuilds.
    #[tokio::test]
    async fn host_mob_restore_is_untouched_by_the_child_policy() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let (svc, mob_id, session) =
            seat_worker_then_exit(&svc, root.path(), false, "restored-host-mob").await;
        let restored = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("runtime custody")
                .with_persistent_storage_root(Some(root.path().to_path_buf()))
                .with_tool_consequence_policy_registry(managed_policy_registry()),
        );
        let worker = AgentIdentity::from("worker-1");
        let status = restored
            .mob_member_status(&mob_id, &worker)
            .await
            .expect("the host mob restores");
        assert_eq!(status.status, meerkat_mob::MobMemberStatus::Active);
        let rebuilt_on = restored
            .mob_resolve_bridge_session_id(&mob_id, &worker)
            .await
            .expect("resolve bridge session")
            .expect("the restored member has a session");
        assert_eq!(
            svc.build_policies_for(&rebuilt_on).last(),
            svc.build_policies_for(&session).last(),
            "the restored member is built with the binding it was seated with"
        );
    }

    #[tokio::test]
    async fn current_session_schedule_survives_member_respawn_and_runtime_restart() {
        use meerkat::surface::SurfaceScheduleMobHost as _;

        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let schedule_path = root.path().join("schedules.db");
        let identity = AgentIdentity::from("worker-1");
        let mut definition = explicit_definition("scheduled-identity-e2e");
        if let Some(meerkat_mob::ProfileBinding::Inline(profile)) =
            definition.profiles.get_mut(&ProfileName::from("worker"))
        {
            profile.external_addressable = true;
        }

        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(runtime_root.clone())),
        );
        let mob_id = state
            .mob_create_definition(definition)
            .await
            .expect("create persistent mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn original worker");
        let original_bridge_session = state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve original bridge session")
            .expect("original bridge session exists");

        let schedule_store = Arc::new(
            meerkat_store::SqliteScheduleStore::open(&schedule_path).expect("open schedule store"),
        );
        let schedule_service = meerkat_schedule::ScheduleService::new(schedule_store.clone());
        let dispatcher = meerkat_schedule::CurrentSessionScheduleToolDispatcher::new_with_resolver(
            Arc::new(meerkat_schedule::ScheduleToolDispatcher::new(
                schedule_service.clone(),
            )),
            original_bridge_session.clone(),
            Arc::new(
                meerkat::surface::MobMemberCurrentSessionScheduleResolver::new(
                    meerkat_core::MobMemberBinding {
                        mob_id: mob_id.to_string(),
                        role: "worker".to_string(),
                        member: identity.to_string(),
                    },
                ),
            ),
        );
        let schedule_args = json!({
            "name": "durable identity schedule",
            "description": "created by an identity-backed member through current_session",
            "trigger": {
                "type": "interval",
                "start_at_utc": chrono::Utc::now(),
                "every_seconds": 60
            },
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "scheduled identity survives restart"
                }
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        });
        let raw_schedule_args =
            serde_json::value::RawValue::from_string(schedule_args.to_string()).expect("raw args");
        let created = dispatcher
            .dispatch(ToolCallView {
                id: "schedule-1",
                name: "meerkat_schedule_create",
                args: raw_schedule_args.as_ref(),
            })
            .await
            .expect("schedule create through current_session wrapper");
        let created: serde_json::Value =
            serde_json::from_str(&created.result.text_content()).expect("created schedule json");
        let schedule_id = meerkat_schedule::ScheduleId::parse(
            created["schedule_id"]
                .as_str()
                .expect("created schedule id"),
        )
        .expect("valid schedule id");
        let persisted = schedule_service
            .get(&schedule_id)
            .await
            .expect("load created schedule");
        let meerkat::TargetBinding::Identity(binding) = &persisted.target else {
            panic!("current_session from mob member must persist as identity target");
        };
        assert!(
            !binding
                .identity()
                .contains(&original_bridge_session.to_string()),
            "durable identity must not include the transient bridge session"
        );
        assert!(
            !binding.identity().contains("worker\""),
            "durable identity must not include the old profile/role"
        );

        state
            .mob_respawn(&mob_id, identity.clone(), None)
            .await
            .expect("respawn worker");
        let respawned_bridge_session = state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve respawned bridge session")
            .expect("respawned bridge session exists");
        assert_ne!(
            respawned_bridge_session, original_bridge_session,
            "respawn should regenerate the bridge session"
        );

        let old_handle = state.handle_for(&mob_id).await.expect("old mob handle");
        old_handle
            .crash_stop_preserving_durable_work_for_test()
            .await
            .expect("crash-stop old mob actor before reopening durable storage");
        let closed_error = tokio::time::timeout(Duration::from_secs(1), old_handle.stop())
            .await
            .expect("old actor command channel closes after crash stop")
            .expect_err("crash-stopped actor must reject later commands");
        assert!(matches!(closed_error, MobError::ActorCommandChannelClosed));
        drop(old_handle);
        drop(state);
        // A process restart rebuilds both the surface sidecars and their
        // machine-owned executor attachments. Retain only the mock session
        // facts needed to stand those actors back up; reusing the old machine
        // here would be a warm surface replacement over a live attachment,
        // which correctly requires its original exact sidecar.
        let svc = Arc::new(svc.cold_restart().await);
        let restored_state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(runtime_root)),
        );
        let restored_bridge_session = restored_state
            .mob_resolve_bridge_session_id(&mob_id, &identity)
            .await
            .expect("resolve restored bridge session")
            .expect("restored bridge session exists");
        assert_ne!(
            restored_bridge_session, respawned_bridge_session,
            "the explicitly nonpersistent session fixture must rotate its bridge session after a cold restart"
        );

        assert_eq!(
            restored_state
                .mob_status(&mob_id)
                .await
                .expect("restored mob status"),
            MobState::Running,
            "fresh session recreation must restore its generated runtime binding before admission"
        );

        let restored_schedule_store = Arc::new(
            meerkat_store::SqliteScheduleStore::open(&schedule_path)
                .expect("reopen schedule store"),
        );
        let restored_schedule_service =
            meerkat_schedule::ScheduleService::new(restored_schedule_store);
        let restored_schedule = restored_schedule_service
            .get(&schedule_id)
            .await
            .expect("load schedule after restart");
        let occurrence = meerkat_schedule::Occurrence::planned_from_schedule(
            &restored_schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            chrono::Utc::now(),
        )
        .expect("plan restored occurrence");
        let meerkat::TargetBinding::Identity(restored_binding) = &restored_schedule.target else {
            panic!("restored schedule must keep identity target");
        };
        let host = MobMcpScheduleHost::new(restored_state);
        let probe = host
            .probe_identity_target(restored_binding)
            .await
            .expect("probe restored identity target")
            .expect("mob identity should be handled by mob host");
        assert!(matches!(probe, meerkat::TargetProbeOutcome::Ready));

        let before_turns = svc.start_turn_call_count();
        let delivery_identity = meerkat::ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let dispatch = host
            .deliver_identity_target(&occurrence, &delivery_identity, restored_binding)
            .await
            .expect("deliver restored identity target")
            .expect("mob identity should deliver through restored mob host");
        let terminal = dispatch.completion.await.expect("delivery completion");
        assert_eq!(
            terminal.phase,
            meerkat::OccurrencePhase::Completed,
            "scheduled identity delivery failed: {terminal:?}"
        );
        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(delivery_identity.correlation_id.as_str())
        );
        let delivery_deadline = Instant::now() + Duration::from_secs(2);
        while svc.start_turn_call_count() <= before_turns {
            assert!(
                Instant::now() < delivery_deadline,
                "accepted scheduled delivery should reach the restored member session"
            );
            sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            svc.start_turn_call_count(),
            before_turns + 1,
            "scheduled prompt should reach the restored current member session"
        );
        let (delivered_session, delivered_prompt) = svc
            .last_start_turn()
            .await
            .expect("scheduled delivery should start a turn");
        assert_eq!(
            delivered_session, restored_bridge_session,
            "scheduled prompt must target the restored current materialized session"
        );
        assert_ne!(
            delivered_session, original_bridge_session,
            "scheduled prompt must not target the original transient bridge session"
        );
        assert_eq!(delivered_prompt, "scheduled identity survives restart");
    }

    #[tokio::test]
    async fn test_mob_destroy_removes_persistent_store_file() {
        let svc = Arc::new(LocalSessionService::new());
        assert!(
            !MobSessionService::supports_persistent_sessions(svc.as_ref()),
            "fixture must prove Mob event-log persistence independently of session-body persistence"
        );
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("destroyed-explicit"))
            .await
            .expect("create explicit mob");
        let storage_root = MobMcpState::persistent_mob_root(root.path());
        let storage_path = MobMcpState::persistent_storage_path(&storage_root, &mob_id);
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "persistent mob db should exist after create"
        );

        state.mob_destroy(&mob_id).await.expect("destroy mob");
        assert!(
            tokio::fs::metadata(&storage_path).await.is_err(),
            "destroy should remove persistent mob db"
        );

        let restored = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        assert!(
            restored
                .mob_list()
                .await
                .expect("list restored mobs")
                .is_empty(),
            "destroyed persistent mobs must not reappear after restart"
        );
    }

    #[tokio::test]
    async fn test_mob_list_surfaces_persistent_restore_failure() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let mob_root = MobMcpState::persistent_mob_root(root.path());
        tokio::fs::create_dir_all(&mob_root)
            .await
            .expect("create mob root");
        tokio::fs::write(
            mob_root.join(format!("{}broken.db", MobMcpState::MOB_STORE_FILE_PREFIX)),
            b"not a sqlite database",
        )
        .await
        .expect("write invalid mob store");

        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let err = state
            .mob_list()
            .await
            .expect_err("restore failure must not be reported as an empty successful list");

        assert!(
            err.to_string().contains("broken.db") || err.to_string().contains("database"),
            "restore failure should preserve the typed storage error, got {err}"
        );
    }

    /// A persisted mob whose resume is refused at restore no longer fails the
    /// whole restore: the other mob is restored and serves, and the refused
    /// one is left unrestored with its store as found. A lookup of its id, a
    /// create of that id included, answers the typed refusal. A later pass
    /// whose resume of that store succeeds no longer lists it.
    #[tokio::test]
    async fn a_store_whose_resume_is_refused_is_left_unrestored_and_the_rest_restores() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let serving = state
            .mob_create_definition(explicit_definition("restore-serving"))
            .await
            .expect("create the mob that restores");
        let refused = state
            .mob_create_definition(explicit_definition("restore-refused"))
            .await
            .expect("create the mob whose resume is refused");
        let refused_store = MobMcpState::persistent_storage_path(
            &MobMcpState::persistent_mob_root(root.path()),
            &refused,
        );
        // A restart: the predecessors give up their supervisor routes.
        for mob_id in [&serving, &refused] {
            state
                .handle_for(mob_id)
                .await
                .expect("predecessor mob handle")
                .shutdown()
                .await
                .expect("retire the predecessor supervisor route");
        }

        let restored = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        *restored
            .refused_resume_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(refused_store.clone());

        let listed = restored
            .mob_list()
            .await
            .expect("the restore goes on past the store whose resume is refused");
        assert!(
            listed.iter().any(|(mob_id, _)| mob_id == &serving),
            "the other mob is restored"
        );
        assert!(
            listed.iter().all(|(mob_id, _)| mob_id != &refused),
            "the refused mob is not managed"
        );
        restored
            .mob_status(&serving)
            .await
            .expect("the restored mob serves");

        let is_refusal = |error: &MobError| {
            matches!(
                error,
                MobError::IncarnationInconsistent {
                    mob_id,
                    inconsistency: meerkat_mob::IncarnationInconsistency::MultipleRecords {
                        count: 2
                    },
                } if mob_id == &refused
            )
        };
        let lookup = restored
            .mob_status(&refused)
            .await
            .expect_err("the refused mob was not restored");
        assert!(is_refusal(&lookup), "{lookup:?}");
        let unrestored = restored.unrestored_mobs().await;
        assert_eq!(unrestored.len(), 1, "{unrestored:?}");
        assert_eq!(unrestored[0].0, refused);
        assert!(is_refusal(&unrestored[0].1), "{:?}", unrestored[0].1);
        let create = restored
            .mob_create_definition(explicit_definition("restore-refused"))
            .await
            .expect_err("a create does not take the unrestored mob's id");
        assert!(is_refusal(&create), "{create:?}");
        assert!(
            tokio::fs::metadata(&refused_store).await.is_ok(),
            "the unrestored mob's store is kept"
        );

        // A later restore pass rebuilds the unrestored set: the store whose
        // resume succeeds now is managed and no longer listed.
        *restored
            .refused_resume_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        restored
            .restore_from_persistent_storage()
            .await
            .expect("a later restore pass");
        let unrestored = restored.unrestored_mobs().await;
        assert!(unrestored.is_empty(), "{unrestored:?}");
        restored
            .mob_status(&refused)
            .await
            .expect("the mob the later pass resumed serves");
    }

    /// A persisted mob left unrestored is still read for member lookups,
    /// and never matched: a session its stored roster names is refused,
    /// naming that mob, and so is a create by that session, before any store
    /// exists for the new id. A session no roster names still gets a
    /// complete, empty lookup, and its create records that session.
    #[tokio::test]
    async fn an_unrestored_mobs_roster_refuses_the_lookup_of_its_member_sessions() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = restored_state(&svc, root.path());
        let refused = state
            .mob_create_definition(explicit_definition("unrestored-parent"))
            .await
            .expect("create the mob whose resume is refused");
        let worker = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &refused,
                ProfileName::from("worker"),
                worker.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn a member of that mob");
        let member_session = state
            .mob_resolve_bridge_session_id(&refused, &worker)
            .await
            .expect("resolve bridge session")
            .expect("member bridge session");
        // A restart: the predecessor gives up its supervisor route.
        state
            .handle_for(&refused)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");

        let mob_root = MobMcpState::persistent_mob_root(root.path());
        let restored = restored_state(&svc, root.path());
        *restored
            .refused_resume_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(MobMcpState::persistent_storage_path(&mob_root, &refused));
        let names_refused = |error: &MobError| {
            matches!(
                error,
                MobError::ProvenanceUnavailable { mob_id: Some(blocking) } if blocking == &refused
            )
        };
        let lookup = restored
            .session_member_matches(&member_session)
            .await
            .expect_err("the unrestored roster names the member's session");
        assert!(names_refused(&lookup), "{lookup:?}");
        let blocked = MobId::from("unrestored-member-create");
        let create = restored
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("unrestored-member-create"),
                member_session,
                false,
                false,
            )
            .await
            .expect_err("a create by that session is refused");
        assert!(names_refused(&create), "{create:?}");
        assert!(
            tokio::fs::metadata(MobMcpState::persistent_storage_path(&mob_root, &blocked))
                .await
                .is_err(),
            "no store exists for the refused create's id"
        );

        let caller = SessionId::new();
        svc.insert_persisted_session(Session::with_id(caller.clone()))
            .await;
        assert!(
            restored
                .session_member_matches(&caller)
                .await
                .expect("a session no roster names gets a complete lookup")
                .is_empty()
        );
        let created = restored
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("unrestored-other-create"),
                caller.clone(),
                false,
                false,
            )
            .await
            .expect("a create by a session no roster names");
        assert_eq!(
            restored
                .handle_for(&created)
                .await
                .expect("handle")
                .provenance()
                .await
                .expect("provenance"),
            meerkat_mob::MobProvenance::Session { session_id: caller }
        );
    }

    /// The restore's empty-store sweep keeps a store whose log is empty but
    /// whose creation record is not finalized: its log lost the `MobCreated`
    /// that record names, so the store is left unrestored under that mob,
    /// typed, and never deleted. An empty store with no record is still
    /// swept.
    #[tokio::test]
    async fn the_restore_sweep_keeps_an_empty_log_whose_creation_record_is_live() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = restored_state(&svc, root.path());
        let damaged = state
            .mob_create_definition(explicit_definition("sweep-live-record"))
            .await
            .expect("create the mob whose log is lost");
        // A restart: the predecessor gives up its supervisor route.
        state
            .handle_for(&damaged)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
        drop(state);
        let mob_root = MobMcpState::persistent_mob_root(root.path());
        let damaged_store = MobMcpState::persistent_storage_path(&mob_root, &damaged);
        MobStorage::drop_event_log_for_test(&damaged_store)
            .expect("drop the log, not through the destroy's clear");
        let empty_store =
            MobMcpState::persistent_storage_path(&mob_root, &MobId::from("sweep-empty"));
        drop(MobStorage::persistent(&empty_store).expect("an empty store with no record"));

        let restored = restored_state(&svc, root.path());
        let unrestored = restored.unrestored_mobs().await;
        assert_eq!(unrestored.len(), 1, "{unrestored:?}");
        assert_eq!(unrestored[0].0, damaged);
        assert!(
            matches!(
                &unrestored[0].1,
                MobError::IncarnationInconsistent {
                    mob_id,
                    inconsistency: meerkat_mob::IncarnationInconsistency::RecordWithoutCreation,
                } if mob_id == &damaged
            ),
            "{:?}",
            unrestored[0].1
        );
        assert!(
            tokio::fs::metadata(&damaged_store).await.is_ok(),
            "the store whose record is not finalized is kept"
        );
        assert!(
            tokio::fs::metadata(&empty_store).await.is_err(),
            "the empty store with no record is swept"
        );
    }

    #[tokio::test]
    async fn test_incomplete_mob_destroy_retains_storage_and_retry_anchor() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );

        let mob_id = state
            .mob_create_definition(explicit_definition("partial-destroy-explicit"))
            .await
            .expect("create explicit mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let retained_handle = state.handle_for(&mob_id).await.expect("mob handle");
        let bridge_session_id = retained_handle
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced archive failure for partial destroy test",
        )
        .await;

        let storage_root = MobMcpState::persistent_mob_root(root.path());
        let storage_path = MobMcpState::persistent_storage_path(&storage_root, &mob_id);
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "persistent mob db should exist before destroy"
        );

        let public_payload = json!({
            "mob_id": mob_id.to_string(),
            "action": "destroy",
        });
        let err = crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &public_payload,
        )
        .await
        .expect_err("incomplete destroy must fail closed");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        );
        let data = err.data.expect("incomplete destroy should include data");
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(
            data.get("destroy_report")
                .and_then(|report| report.get("errors"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|errors| !errors.is_empty()),
            "partial destroy error data should include the destroy report errors: {data}"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "incomplete destroy must retain persistent mob db"
        );
        assert!(
            svc.session_exists(&bridge_session_id).await,
            "failed ArchiveSession cleanup must leave the bridge session available for retry"
        );
        let members = state
            .mob_list_members(&mob_id)
            .await
            .expect("list members after incomplete destroy");
        assert!(
            members
                .iter()
                .any(|member| member.agent_identity == "worker-1"),
            "incomplete destroy must retain the failed member as retry work"
        );
        let mobs = state.mob_list().await.expect("list retained mobs");
        assert!(
            mobs.iter().any(|(id, _)| id == &mob_id),
            "incomplete destroy must retain in-memory retry anchor"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        // The retained handle still owns the mob database's observer: the
        // native destroy completes, and its storage cleanup stays owed.
        let incomplete = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("a retained handle keeps the storage cleanup outstanding");
        let MobMcpDestroyError::Incomplete { report: annotated } = incomplete else {
            panic!("only the storage cleanup may be outstanding: {incomplete:?}");
        };
        assert!(
            annotated
                .errors
                .iter()
                .any(|error| error.starts_with("storage cleanup outstanding: ")),
            "the answer names the outstanding cleanup: {annotated:?}"
        );
        assert!(
            !svc.session_exists(&bridge_session_id).await,
            "complete retry must actually archive the bridge session before storage removal"
        );
        assert!(
            state
                .mob_list()
                .await
                .expect("list mobs after retry")
                .is_empty(),
            "complete retry should remove retry anchor"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "storage a live handle still owns is kept"
        );
        retained_handle
            .poll_events(0, 1)
            .await
            .expect("the retained handle still reads its storage");

        drop(retained_handle);
        let retry_report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the same caller's retry completes the storage cleanup");
        assert!(retry_report.metadata_scrubbed);
        assert!(retry_report.events_cleared);
        assert!(retry_report.namespace_cleaned);
        assert!(
            retry_report.errors.is_empty(),
            "the canonical report carries no stale cleanup error: {retry_report:?}"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_err(),
            "complete retry should remove persistent mob db"
        );
    }

    /// The mob database path, under a persistent root.
    fn destroy_test_storage_path(root: &std::path::Path, mob_id: &MobId) -> PathBuf {
        MobMcpState::persistent_storage_path(&MobMcpState::persistent_mob_root(root), mob_id)
    }

    /// Which of a mob database and its sidecars exist.
    fn existing_storage_files(path: &std::path::Path) -> Vec<PathBuf> {
        MobMcpState::storage_file_paths(path)
            .into_iter()
            .filter(|path| std::fs::symlink_metadata(path).is_ok())
            .collect()
    }

    fn persistent_destroy_state(
        svc: &Arc<MockSessionSvc>,
        root: &std::path::Path,
    ) -> Arc<MobMcpState> {
        Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.to_path_buf())),
        )
    }

    /// Generated mob operator authority that manages `mob_id` at
    /// `incarnation`, with a fresh principal token.
    fn managing_authority(
        mob_id: &MobId,
        incarnation: meerkat_core::service::LiveMobIncarnation,
    ) -> meerkat_core::service::MobToolAuthorityContext {
        let authority =
            meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
                .expect("generated create-only mob authority");
        meerkat_runtime::mob_operator_authority::grant_manage_mob(
            &authority,
            mob_id.as_str(),
            incarnation,
        )
        .expect("generated managed mob scope")
    }

    /// The incarnation the live mob `mob_id` runs at, as its handle's
    /// witness.
    async fn live_incarnation(
        state: &MobMcpState,
        mob_id: &MobId,
    ) -> meerkat_core::service::LiveMobIncarnation {
        state
            .handle_for(mob_id)
            .await
            .expect("live mob handle")
            .live_incarnation()
    }

    fn destroy_surface(
        state: &Arc<MobMcpState>,
        bridge_session: SessionId,
        authority: meerkat_core::service::MobToolAuthorityContext,
    ) -> AgentMobToolSurface {
        AgentMobToolSurface::new_with_effective_authority(
            Arc::clone(state),
            None,
            Arc::new(std::sync::RwLock::new(authority)),
            "claude-sonnet-4-5".to_string(),
            bridge_session,
            None,
            None,
            None,
            meerkat_core::service::MobToolSnapshotContext::Standalone,
        )
    }

    async fn dispatch_destroy(
        surface: &AgentMobToolSurface,
        mob_id: &MobId,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args =
            serde_json::value::RawValue::from_string(json!({ "mob_id": mob_id }).to_string())
                .expect("raw args");
        surface
            .dispatch(ToolCallView {
                id: "destroy-call",
                name: "mob_destroy",
                args: &args,
            })
            .await
    }

    /// Spawn a worker and park the destroy's archive of its session.
    async fn spawn_worker_with_held_archive(
        svc: &MockSessionSvc,
        state: &MobMcpState,
        mob_id: &MobId,
    ) -> (Arc<InsertHold>, meerkat_mob::MobActorTaskWatch) {
        state
            .mob_spawn(
                mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let handle = state.handle_for(mob_id).await.expect("mob handle");
        let worker_session = handle
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        let actor_task = handle.actor_task_watch();
        drop(handle);
        (svc.hold_archive(worker_session), actor_task)
    }

    async fn wait_until_reached(hold: &InsertHold) {
        while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The record a destroy left, once it exists.
    async fn recorded_destroy(state: &MobMcpState, mob_id: &MobId) -> Arc<RetiredMob> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let recorded = state
                    .retired_mobs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(mob_id)
                    .cloned();
                if let Some(recorded) = recorded {
                    return recorded;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the admitted destroy records its report without its caller")
    }

    /// A held public reader of the mob database keeps the destroy's storage
    /// cleanup outstanding: the destroy answers `Incomplete` with a copy of
    /// the report naming it, the files and the reader stay, and once the
    /// reader is gone the same caller's retry removes the files and answers
    /// the canonical report, again on a later retry.
    #[tokio::test]
    async fn a_held_reader_keeps_destroy_storage_cleanup_outstanding_until_the_same_caller_retries()
    {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("held-reader-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");

        let payload = json!({ "mob_id": mob_id.to_string(), "action": "destroy" });
        let error =
            crate::public_mcp::handle_public_tools_call(&state, "meerkat_mob_lifecycle", &payload)
                .await
                .expect_err("the held reader keeps the storage cleanup outstanding");
        let data = error.data.expect("incomplete destroy data");
        assert_eq!(data["code"], "mob_destroy_incomplete");
        assert_eq!(data["retryable"], true);
        let mut canonical = data["destroy_report"].clone();
        let outstanding = canonical["errors"]
            .as_array_mut()
            .and_then(Vec::pop)
            .expect("the outstanding cleanup is named");
        // A report without errors serializes no `errors` field.
        if canonical["errors"].as_array().is_some_and(Vec::is_empty) {
            canonical
                .as_object_mut()
                .expect("report object")
                .remove("errors");
        }
        assert!(
            outstanding.as_str().is_some_and(|outstanding| {
                outstanding.starts_with("storage cleanup outstanding: ")
                    && outstanding.contains("owner handle")
            }),
            "{data}"
        );
        assert!(
            state.mob_list().await.expect("list mobs").is_empty(),
            "the native destroy completed"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "the held reader's database is kept"
        );
        reader
            .is_event_log_empty()
            .await
            .expect("the held reader still reads");

        drop(reader);
        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the same caller's retry completes the cleanup");
        assert_eq!(
            serde_json::to_value(&report).expect("report json"),
            canonical,
            "the retry answers the canonical report, without the cleanup annotation"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
        let again = state
            .mob_destroy(&mob_id)
            .await
            .expect("a later retry by the same caller collects the report again");
        assert_eq!(
            serde_json::to_value(&again).expect("report json"),
            canonical
        );
    }

    /// A host binding, as an activation hook takes one: every mob that
    /// activates leaves its read handle in the returned map.
    fn bind_activated_mobs(
        state: &MobMcpState,
    ) -> Arc<std::sync::Mutex<BTreeMap<MobId, meerkat_mob::MobReadHandle>>> {
        let bound = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        let binder = Arc::clone(&bound);
        let hook: meerkat_mob::MobBeforeActivation = Arc::new(move |handle| {
            binder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(handle.mob_id().clone(), handle);
            Box::pin(async { Ok(()) })
        });
        state
            .set_before_activation(hook)
            .expect("bind the activation hook");
        bound
    }

    /// The after-destroy hook gets the destroyed mob's id once the native
    /// destroy completed (the host's read handle observes `Destroyed`) and
    /// before the storage cleanup that destroy owes (the database is still
    /// there). A host that drops the read handle there leaves nothing
    /// outstanding: the destroy removes the mob's files.
    #[tokio::test]
    async fn an_after_destroy_hook_releases_a_host_binding_before_the_storage_cleanup() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let bound = bind_activated_mobs(&state);
        let released = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (releaser, seen) = (Arc::clone(&bound), Arc::clone(&released));
        let storage_root = root.path().to_path_buf();
        let hook: MobAfterDestroy = Arc::new(move |mob_id: &MobId| {
            let handle = releaser
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(mob_id)
                .expect("the host bound the mob it is told was destroyed");
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    mob_id.clone(),
                    handle.status_observation_snapshot(),
                    destroy_test_storage_path(&storage_root, mob_id).exists(),
                ));
        });
        state
            .set_after_destroy(hook)
            .expect("bind the destroy hook");
        assert!(
            state.set_after_destroy(Arc::new(|_: &MobId| {})).is_err(),
            "the destroy hook binds once"
        );
        let mob_id = state
            .mob_create_definition(explicit_definition("released-binding-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        assert!(
            bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&mob_id),
            "the activation hook bound the mob"
        );

        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the released binding leaves no storage cleanup outstanding");
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(
            *released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![(mob_id.clone(), meerkat_mob::MobState::Destroyed, true)],
            "the hook ran once, after the native destroy and before the cleanup"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// A host that still holds the mob when the after-destroy hook first
    /// runs, and through every retried drain, keeps the storage cleanup
    /// outstanding. The same caller's retry
    /// runs the hook again before its own cleanup attempt, and once the host
    /// releases there, the retry removes the mob's files.
    #[tokio::test]
    async fn a_destroy_cleanup_retry_runs_the_after_destroy_hook_again() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let bound = bind_activated_mobs(&state);
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (releaser, releasing, counted) =
            (Arc::clone(&bound), Arc::clone(&release), Arc::clone(&calls));
        let hook: MobAfterDestroy = Arc::new(move |mob_id: &MobId| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if releasing.load(std::sync::atomic::Ordering::SeqCst) {
                releaser
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(mob_id);
            }
        });
        state
            .set_after_destroy(hook)
            .expect("bind the destroy hook");
        let mob_id = state
            .mob_create_definition(explicit_definition("held-binding-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);

        let incomplete = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the host's held binding keeps the storage cleanup outstanding");
        let MobMcpDestroyError::Incomplete { report } = incomplete else {
            panic!("only the storage cleanup may be outstanding: {incomplete:?}");
        };
        assert!(
            report.errors.iter().any(|error| {
                error.starts_with("storage cleanup outstanding: ") && error.contains("owner handle")
            }),
            "{report:?}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1 + OBSERVER_DRAIN_RETRIES,
            "the hook ran before the cleanup and again before each retried drain"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "the held database is kept"
        );

        release.store(true, std::sync::atomic::Ordering::SeqCst);
        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the retry releases the binding first and completes the cleanup");
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            2 + OBSERVER_DRAIN_RETRIES,
            "the retry ran the hook again before its cleanup"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
        assert!(
            bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    /// A panicking after-destroy hook is contained. The hook runs once the
    /// destroyed mob left the managed set, so the panic cannot keep its id
    /// managed; the destroy goes on with its storage cleanup, and the id can
    /// be created again at once.
    #[tokio::test]
    async fn a_panicking_after_destroy_hook_neither_keeps_the_id_nor_skips_the_cleanup() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let still_managed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (observer, seen) = (Arc::downgrade(&state), Arc::clone(&still_managed));
        let hook: MobAfterDestroy = Arc::new(move |mob_id: &MobId| {
            let managed = observer.upgrade().is_some_and(|state| {
                state
                    .mobs
                    .try_read()
                    .map_or(true, |mobs| mobs.contains_key(mob_id))
            });
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(managed);
            panic!("the host's release failed");
        });
        state
            .set_after_destroy(hook)
            .expect("bind the destroy hook");
        let mob_id = state
            .mob_create_definition(explicit_definition("panicking-release-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);

        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("a panicking hook does not fail the destroy");
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(
            *still_managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![false],
            "the hook ran once, after the destroyed mob left the managed set"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
        state
            .mob_create_definition(explicit_definition("panicking-release-destroy"))
            .await
            .expect("the destroyed mob's id is free again at once");
    }

    /// A holder that still observes the destroyed mob's database when the
    /// cleanup first drains it, and lets go before the retry, does not leave
    /// the cleanup outstanding: the refused drain is retried a bounded number
    /// of times, each after the host's release hook ran again. The holder
    /// lets go when the hook runs again, not after a fixed delay: a delay
    /// races the first drain when all of the binary's tests run at once
    /// (plain `cargo test`), and that drain then has nothing to refuse.
    #[tokio::test]
    async fn a_short_lived_holder_does_not_leave_the_destroy_cleanup_outstanding() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let bound = bind_activated_mobs(&state);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let holder = Arc::new(std::sync::Mutex::new(None));
        let (releaser, counted, slot) =
            (Arc::clone(&bound), Arc::clone(&calls), Arc::clone(&holder));
        let hook: MobAfterDestroy = Arc::new(move |mob_id: &MobId| {
            let mut held = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                // The first call hands the binding to a short-lived holder,
                // as an observer that cloned it just before the destroy would
                // be.
                *held = releaser
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(mob_id);
            } else {
                // The holder lets go before the retried drain.
                drop(held.take());
            }
        });
        state
            .set_after_destroy(hook)
            .expect("bind the destroy hook");
        let mob_id = state
            .mob_create_definition(explicit_definition("short-lived-holder-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);

        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the retried drain settles once the holder let go");
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(
            calls.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "the hook ran again before a retried drain"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// A mob whose phase is `Destroyed` is not handed out by
    /// `mob_handles_snapshot` while its managed entry remains, as after a
    /// destroy through the mob's own handle: a snapshot taken then would hold
    /// the destroyed mob's database observer. `mob_list` still lists it, as
    /// `Destroyed`.
    #[tokio::test]
    async fn mob_handles_snapshot_skips_a_destroyed_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let kept = state
            .mob_create_definition(explicit_definition("snapshot-kept"))
            .await
            .expect("create the kept mob");
        let destroyed = state
            .mob_create_definition(explicit_definition("snapshot-destroyed"))
            .await
            .expect("create the destroyed mob");
        let handle = state
            .handle_for(&destroyed)
            .await
            .expect("the destroyed mob's handle");
        handle
            .destroy()
            .await
            .expect("destroy through the mob's own handle");
        assert_eq!(
            handle.status_observation_snapshot(),
            meerkat_mob::MobState::Destroyed
        );
        drop(handle);

        let snapshot: Vec<MobId> = state
            .mob_handles_snapshot()
            .await
            .expect("owner snapshot")
            .into_iter()
            .map(|(mob_id, _)| mob_id)
            .collect();
        assert_eq!(snapshot, vec![kept.clone()]);
        let listed: BTreeMap<MobId, meerkat_mob::MobState> = state
            .mob_list()
            .await
            .expect("owner list")
            .into_iter()
            .map(|(mob_id, observation)| (mob_id, observation.phase))
            .collect();
        assert_eq!(
            listed.keys().cloned().collect::<Vec<_>>(),
            vec![destroyed.clone(), kept]
        );
        assert_eq!(listed[&destroyed], meerkat_mob::MobState::Destroyed);
    }

    /// A managed mob whose actor fail-stopped in its startup recovery, with
    /// the cause `status()` reports for it.
    async fn managed_mob_whose_actor_fail_stopped_at_start(
        state: &Arc<MobMcpState>,
        mob_id: &str,
    ) -> (MobId, Arc<MobError>) {
        let mob_id = MobId::from(mob_id);
        meerkat_mob::fail_stop_next_actor_start_for_test(&mob_id);
        let created = state
            .mob_create_definition(explicit_definition(mob_id.as_str()))
            .await
            .expect("create the mob");
        assert_eq!(created, mob_id);
        let handle = state.handle_for(&mob_id).await.expect("the mob's handle");
        assert_eq!(
            handle.actor_task_watch().wait().await,
            meerkat_mob::MobActorTaskWait::Returned,
            "the actor fail-stops in its startup recovery"
        );
        match handle.status().await {
            Err(MobError::SharedLifecycleFailure(cause)) => (mob_id, cause),
            other => panic!("a fail-stopped actor reports why, typed: {other:?}"),
        }
    }

    /// `mob_list` lists a mob whose actor fail-stopped with why it stopped,
    /// the cause `status()` reports; its phase stays the last one the actor
    /// published.
    #[tokio::test]
    async fn mob_list_lists_a_fail_stopped_mob_with_why_its_actor_stopped() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let live = state
            .mob_create_definition(explicit_definition("listed-live"))
            .await
            .expect("create the live mob");
        let (stopped, reported) =
            managed_mob_whose_actor_fail_stopped_at_start(&state, "listed-fail-stopped").await;

        let listed: BTreeMap<MobId, meerkat_mob::MobLifecycleObservation> = state
            .mob_list()
            .await
            .expect("owner list")
            .into_iter()
            .collect();
        assert!(
            listed[&live].failure.is_none(),
            "a live mob is listed without a failure: {:?}",
            listed[&live]
        );
        let observation = &listed[&stopped];
        assert!(
            matches!(
                observation.phase,
                meerkat_mob::MobState::Creating | meerkat_mob::MobState::Running
            ),
            "the phase stays the last one the actor published: {observation:?}"
        );
        let failure = observation.failure.as_ref().unwrap_or_else(|| {
            panic!("a fail-stopped mob is listed with why its actor stopped: {observation:?}")
        });
        assert!(
            Arc::ptr_eq(failure, &reported),
            "the listed failure is the cause status() reports: {failure:?} vs {reported:?}"
        );
    }

    /// `mob_handles_snapshot` still hands out a mob whose actor fail-stopped:
    /// hiding it would make the failure silently absent from the observation
    /// surfaces built on the snapshot, which read why from the handle.
    #[tokio::test]
    async fn mob_handles_snapshot_still_hands_out_a_fail_stopped_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let (stopped, _reported) =
            managed_mob_whose_actor_fail_stopped_at_start(&state, "snapshot-fail-stopped").await;

        let snapshot = state.mob_handles_snapshot().await.expect("owner snapshot");
        let (_, handle) = snapshot
            .iter()
            .find(|(mob_id, _)| *mob_id == stopped)
            .expect("the fail-stopped mob's handle is handed out");
        assert!(
            matches!(
                handle.status().await,
                Err(MobError::SharedLifecycleFailure(_))
            ),
            "a reader of the handed-out handle gets why its actor stopped"
        );
    }

    /// A host's create hook is asked before anything of a new mob exists, on
    /// every create path, with what that path itself knows: the owner
    /// session, and whether the mob is that session's implicit delegation
    /// mob. A refusal leaves nothing of the mob behind and is recognizable,
    /// with the host's reason in its message and its structured data.
    #[tokio::test]
    async fn a_before_create_hook_answers_before_anything_of_the_mob_exists() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&asked);
        let hook: MobBeforeCreate = Arc::new(move |request: MobCreateRequest| {
            let refuse = request.mob_id.as_str().starts_with("refused-");
            recorder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            Box::pin(async move {
                if refuse {
                    Err("the host keeps this id".to_string())
                } else {
                    Ok(())
                }
            })
        });
        state.set_before_create(hook).expect("bind the create hook");
        assert!(
            state
                .set_before_create(Arc::new(|_: MobCreateRequest| Box::pin(async { Ok(()) })))
                .is_err(),
            "the create hook binds once"
        );

        let refused_id = MobId::from("refused-host-create");
        let refused = state
            .mob_create_definition(explicit_definition(refused_id.as_str()))
            .await
            .expect_err("the hook refuses the create");
        assert!(MobMcpState::is_create_refusal(&refused), "{refused}");
        assert!(
            refused.to_string().contains("the host keeps this id"),
            "{refused}"
        );
        assert_eq!(
            refused
                .structured_data()
                .and_then(|data| data.get("reason").cloned()),
            Some(json!("the host keeps this id"))
        );
        assert!(
            existing_storage_files(&destroy_test_storage_path(root.path(), &refused_id)).is_empty(),
            "nothing of the refused mob was stored"
        );
        assert!(state.handle_for(&refused_id).await.is_err());

        let owner = SessionId::new();
        let child = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("admitted-child"),
                owner.clone(),
                true,
                false,
            )
            .await
            .expect("the hook admits the child");
        let (implicit, created) = state
            .ensure_implicit_mob_for_model(&owner.to_string(), "claude-sonnet-4-5", None)
            .await
            .expect("the hook admits the implicit delegation mob");
        assert!(created);
        // The implicit flag counts only under the id derived from the owner
        // session.
        let flagged = state
            .mob_create_definition_with_owner_bridge_session(
                explicit_definition("flagged-not-implicit"),
                owner.clone(),
                true,
                true,
            )
            .await
            .expect("the hook admits the flagged create");
        assert_eq!(
            *asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![
                MobCreateRequest {
                    mob_id: refused_id,
                    owner_bridge_session_id: None,
                    implicit_delegation_mob: false,
                },
                MobCreateRequest {
                    mob_id: child,
                    owner_bridge_session_id: Some(owner.clone()),
                    implicit_delegation_mob: false,
                },
                MobCreateRequest {
                    mob_id: implicit,
                    owner_bridge_session_id: Some(owner.clone()),
                    implicit_delegation_mob: true,
                },
                MobCreateRequest {
                    mob_id: flagged,
                    owner_bridge_session_id: Some(owner),
                    implicit_delegation_mob: false,
                },
            ]
        );
    }

    /// The create hook fails closed: a panic while it is called, or in the
    /// future it returns, refuses the create, and nothing of the mob exists.
    #[tokio::test]
    async fn a_panicking_before_create_hook_refuses_the_create() {
        for in_future in [false, true] {
            let svc = Arc::new(MockSessionSvc::new());
            let root = tempfile::tempdir().expect("tempdir");
            let state = persistent_destroy_state(&svc, root.path());
            let hook: MobBeforeCreate = Arc::new(move |request: MobCreateRequest| {
                let id = request.mob_id.to_string();
                assert!(
                    in_future || id.is_empty(),
                    "the host's admission of {id} failed"
                );
                Box::pin(async move {
                    if id.is_empty() {
                        Ok(())
                    } else {
                        panic!("the host's admission of {id} failed")
                    }
                })
            });
            state.set_before_create(hook).expect("bind the create hook");
            let mob_id = MobId::from("panicking-admission");
            let refused = state
                .mob_create_definition(explicit_definition(mob_id.as_str()))
                .await
                .expect_err("a panicking hook refuses the create");
            assert!(
                MobMcpState::is_create_refusal(&refused),
                "in_future={in_future}: {refused}"
            );
            assert!(
                existing_storage_files(&destroy_test_storage_path(root.path(), &mob_id)).is_empty(),
                "in_future={in_future}: nothing of the refused mob was stored"
            );
            assert!(state.handle_for(&mob_id).await.is_err());
        }
    }

    /// Restoring a persisted mob creates nothing, so it never asks the
    /// create hook: a hook that refuses every create still lets the mob
    /// come back.
    #[tokio::test]
    async fn restoring_a_mob_never_asks_the_before_create_hook() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("restored-without-admission"))
            .await
            .expect("create explicit mob");
        state
            .handle_for(&mob_id)
            .await
            .expect("predecessor mob handle")
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");

        let restored = persistent_destroy_state(&svc, root.path());
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&asked);
        restored
            .set_before_create(Arc::new(move |_: MobCreateRequest| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { Err("this host admits no create".to_string()) })
            }))
            .expect("bind the create hook");
        restored
            .handle_for(&mob_id)
            .await
            .expect("the persisted mob restores");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// An agent's destroy is continued only by the exact caller it was
    /// admitted for. A surface with the same control principal and bridge
    /// session but another mob operator authority is denied, so is one with
    /// the same authority token on another bridge session, and the console
    /// never continues an agent's destroy; none of them removes anything.
    #[tokio::test]
    async fn an_agent_destroy_continues_only_for_its_exact_admitted_caller() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("agent-destroy-caller"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let bridge_session = SessionId::new();
        let incarnation = live_incarnation(&state, &mob_id).await;
        let authority = managing_authority(&mob_id, incarnation);
        let admitted = destroy_surface(&state, bridge_session.clone(), authority.clone());
        let other_authority = destroy_surface(
            &state,
            bridge_session.clone(),
            managing_authority(&mob_id, incarnation),
        );
        let other_session = destroy_surface(&state, SessionId::new(), authority);
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");

        let error = dispatch_destroy(&admitted, &mob_id)
            .await
            .expect_err("the held reader keeps the storage cleanup outstanding");
        let data = error.structured_data().expect("incomplete destroy data");
        assert_eq!(data["code"], "mob_destroy_incomplete", "{error:?}");
        drop(reader);

        for (surface, caller) in [
            (&other_authority, "another mob operator authority"),
            (&other_session, "another bridge session"),
        ] {
            let refused = dispatch_destroy(surface, &mob_id).await.expect_err(caller);
            assert!(
                matches!(refused, ToolError::AccessDenied { .. }),
                "{caller}: {refused:?}"
            );
        }
        let console = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the console never continues an agent's destroy");
        assert!(
            matches!(console, MobMcpDestroyError::Mob(MobError::ScopeDenied(_))),
            "{console:?}"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "a refused caller removes nothing"
        );

        let completed = dispatch_destroy(&admitted, &mob_id)
            .await
            .expect("the exact admitted caller completes the cleanup");
        let body: serde_json::Value =
            serde_json::from_str(&completed.result.text_content()).expect("destroy JSON");
        assert_eq!(body["ok"], true);
        assert!(
            body["destroy_report"]["errors"]
                .as_array()
                .is_none_or(Vec::is_empty),
            "the canonical report carries no stale cleanup error: {body}"
        );
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// One session's agent mob tool surface over a shared state, with the
    /// effective authority its turn owner writes: [`Self::call`] commits the
    /// session effects a dispatch returns, as the agent loop does after the
    /// tool batch.
    struct OperatorSession {
        surface: AgentMobToolSurface,
        authority: Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>,
        session_id: SessionId,
    }

    impl OperatorSession {
        /// A session that may create mobs and manages none.
        fn new(state: &Arc<MobMcpState>) -> Self {
            let authority = Arc::new(std::sync::RwLock::new(
                meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
                    .expect("generated create-only mob authority"),
            ));
            let session_id = SessionId::new();
            let surface = AgentMobToolSurface::new_with_effective_authority(
                Arc::clone(state),
                None,
                Arc::clone(&authority),
                "claude-sonnet-4-5".to_string(),
                session_id.clone(),
                None,
                None,
                None,
                meerkat_core::service::MobToolSnapshotContext::Standalone,
            );
            Self {
                surface,
                authority,
                session_id,
            }
        }

        async fn call(
            &self,
            tool: &str,
            args: serde_json::Value,
        ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
            let args =
                serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
            let outcome = self
                .surface
                .dispatch(ToolCallView {
                    id: "operator-call",
                    name: tool,
                    args: &args,
                })
                .await?;
            for effect in &outcome.session_effects {
                if let meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext {
                    authority_context,
                } = effect
                {
                    *self
                        .authority
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        authority_context.clone();
                }
            }
            Ok(outcome)
        }

        async fn create(&self, mob_id: &str) {
            let definition = json!({
                "id": mob_id,
                "profiles": {
                    "worker": {
                        "model": "claude-sonnet-4-5",
                        "tools": { "comms": true },
                        "peer_description": "worker",
                        "runtime_mode": "turn_driven"
                    }
                }
            });
            if let Err(error) = self
                .call("mob_create", json!({ "definition": definition }))
                .await
            {
                panic!("mob_create {mob_id}: {error:?}");
            }
        }

        /// Each management tool this session might use on `mob_id` is
        /// refused with the ordinary access denial.
        async fn assert_manages_nothing_of(&self, mob_id: &str) {
            for (tool, args) in [
                ("mob_list_members", json!({ "mob_id": mob_id })),
                (
                    "mob_spawn_member",
                    json!({ "mob_id": mob_id, "profile": "worker", "member_id": "intruder" }),
                ),
                ("mob_destroy", json!({ "mob_id": mob_id })),
            ] {
                let refused = self.call(tool, args).await.expect_err(tool);
                assert!(
                    matches!(refused, ToolError::AccessDenied { .. }),
                    "{tool}: {refused:?}"
                );
            }
        }
    }

    /// A session that created and destroyed mob X holds no authority over a
    /// mob another session later creates under the same id: each management
    /// attempt is the ordinary access refusal, and the new mob stays.
    #[tokio::test]
    async fn a_destroyed_mobs_creator_does_not_manage_a_later_mob_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "reused-mob-id";
        first.create(mob_id).await;
        first
            .call("mob_destroy", json!({ "mob_id": mob_id }))
            .await
            .expect("the creator destroys its mob");
        second.create(mob_id).await;

        first.assert_manages_nothing_of(mob_id).await;
        second
            .call("mob_list_members", json!({ "mob_id": mob_id }))
            .await
            .expect("the new creator manages its mob");
        assert!(
            state.handle_for(&MobId::from(mob_id)).await.is_ok(),
            "the refused destroy left the new mob in place"
        );
    }

    /// A session that destroys its mob and creates one again under the same
    /// id manages the new mob: the create grants it afresh.
    #[tokio::test]
    async fn a_creator_that_recreates_its_mob_manages_the_new_one() {
        let state = MobMcpState::new_in_memory();
        let creator = OperatorSession::new(&state);
        let mob_id = "recreated-mob-id";
        creator.create(mob_id).await;
        creator
            .call("mob_destroy", json!({ "mob_id": mob_id }))
            .await
            .expect("the creator destroys its mob");
        creator.create(mob_id).await;

        creator
            .call("mob_list_members", json!({ "mob_id": mob_id }))
            .await
            .expect("the creator manages the mob it created again");
        creator
            .call("mob_destroy", json!({ "mob_id": mob_id }))
            .await
            .expect("the creator destroys the mob it created again");
    }

    /// A grant does not outlive its mob when another caller destroys it:
    /// after the console destroys a session's mob and another session
    /// creates one under the same id, the first session is refused.
    #[tokio::test]
    async fn a_console_destroy_leaves_the_creator_no_authority_over_a_later_mob_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "console-destroyed-mob-id";
        first.create(mob_id).await;
        state
            .mob_destroy(&MobId::from(mob_id))
            .await
            .expect("the console destroys the mob");
        second.create(mob_id).await;

        first.assert_manages_nothing_of(mob_id).await;
        assert!(state.handle_for(&MobId::from(mob_id)).await.is_ok());
    }

    /// A destroy that stops midway leaves its creator's authority as it was:
    /// the same caller's retry completes it, and the grant then reaches no
    /// mob created later under the id.
    #[tokio::test]
    async fn an_incomplete_destroy_leaves_no_authority_over_a_later_mob_under_its_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "incomplete-destroy-mob-id";
        first.create(mob_id).await;
        let storage_path = destroy_test_storage_path(root.path(), &MobId::from(mob_id));
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");

        let incomplete = first
            .call("mob_destroy", json!({ "mob_id": mob_id }))
            .await
            .expect_err("the held reader keeps the storage cleanup outstanding");
        let data = incomplete
            .structured_data()
            .expect("incomplete destroy data");
        assert_eq!(data["code"], "mob_destroy_incomplete", "{incomplete:?}");
        drop(reader);
        first
            .call("mob_destroy", json!({ "mob_id": mob_id }))
            .await
            .expect("the creator's retry completes its destroy");
        second.create(mob_id).await;

        first.assert_manages_nothing_of(mob_id).await;
        assert!(state.handle_for(&MobId::from(mob_id)).await.is_ok());
    }

    /// A mob another session creates first under a session's implicit
    /// delegation mob id gives that session's delegate no scope over it. The
    /// occupied id refuses the delegate's own implicit-mob create, so the
    /// delegate fails before it grants anything (as it did before grants were
    /// bound to incarnations); the delegate grants only an incarnation its
    /// own create built.
    #[tokio::test]
    async fn a_mob_created_under_a_sessions_implicit_id_gives_its_delegate_no_scope() {
        let state = MobMcpState::new_in_memory();
        let owner = OperatorSession::new(&state);
        let squatter = OperatorSession::new(&state);
        let implicit_id = format!("implicit-{}", owner.session_id);
        squatter.create(&implicit_id).await;

        let refused = owner
            .call(
                "delegate",
                json!({
                    "task": "say hi",
                    "member_id": "helper",
                    "result_label": "delegate_result",
                    "max_text_bytes": 4096
                }),
            )
            .await
            .expect_err("the occupied implicit id refuses the delegate's create");
        assert!(
            matches!(refused, ToolError::ExecutionFailed { .. }),
            "{refused:?}"
        );
        {
            let authority = owner
                .authority
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(authority.managed_mob_scope().is_empty());
            assert!(authority.spawn_profile_scope().is_empty());
        }
        owner.assert_manages_nothing_of(&implicit_id).await;
        squatter
            .call("mob_list_members", json!({ "mob_id": implicit_id }))
            .await
            .expect("the creator still manages its mob");
    }

    /// After a restart a session's implicit delegation mob keeps its
    /// incarnation: the delegate's guard still knows it as the session's
    /// own, and manage scope bound to it before the restart names the
    /// restored mob. Grants are not persisted, so after a real restart the
    /// session's next delegate grants that scope again.
    #[tokio::test]
    async fn a_restored_implicit_mob_keeps_its_incarnation_and_stays_the_sessions_own() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let owner = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner.clone()))
            .await;
        let state = restored_state(&svc, root.path());
        let (mob_id, created) = state
            .ensure_implicit_mob_for_model(&owner.to_string(), "claude-sonnet-4-5", None)
            .await
            .expect("create the implicit mob");
        assert!(created);
        let before = state
            .handle_for(&mob_id)
            .await
            .expect("implicit mob handle");
        let before_incarnation = before.live_incarnation();
        before
            .shutdown()
            .await
            .expect("retire the predecessor supervisor route");
        drop(before);
        let svc = Arc::new(svc.cold_restart().await);
        svc.script_observation(
            &owner,
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed { revision: None }),
        );

        let restarted = restored_state(&svc, root.path());
        let restored = restarted
            .handle_for(&mob_id)
            .await
            .expect("restored implicit mob");
        assert_eq!(restored.incarnation(), before_incarnation.incarnation());
        assert!(AgentMobToolSurface::is_implicit_mob_of(&restored, &owner));
        assert!(
            managing_authority(&mob_id, before_incarnation)
                .can_manage_mob(mob_id.as_str(), restored.incarnation())
        );
    }

    /// Park the next agent mob tool call on `state` between its admission
    /// and its action; the hold reports the park and releases it.
    fn hold_next_agent_call(state: &MobMcpState) -> Arc<InsertHold> {
        let hold = Arc::new(InsertHold::default());
        *state
            .agent_call_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hold));
        hold
    }

    async fn wait_parked(hold: &InsertHold) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the call parks between its admission and its action");
    }

    /// Run `operator`'s call of `tool` on the mob `mob_id` and, while it is
    /// parked after its admission, let the console destroy that mob and
    /// `recreator` create one under the same id with a member `b-member`.
    async fn race_call_against_recreate(
        state: &Arc<MobMcpState>,
        operator: &OperatorSession,
        recreator: &OperatorSession,
        mob_id: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let hold = hold_next_agent_call(state);
        let call = operator.call(tool, args);
        let race = async {
            wait_parked(&hold).await;
            state
                .mob_destroy(&MobId::from(mob_id))
                .await
                .expect("the console destroys the admitted mob");
            recreator.create(mob_id).await;
            recreator
                .call(
                    "mob_spawn_member",
                    json!({ "mob_id": mob_id, "profile": "worker", "member_id": "b-member" }),
                )
                .await
                .expect("the new creator spawns into its mob");
            hold.release.notify_one();
        };
        let (outcome, ()) = tokio::join!(call, race);
        outcome
    }

    async fn member_ids(state: &MobMcpState, mob_id: &str) -> Vec<String> {
        state
            .handle_for(&MobId::from(mob_id))
            .await
            .expect("the mob under the id")
            .list_members_including_retiring()
            .await
            .into_iter()
            .map(|member| member.agent_identity.to_string())
            .collect()
    }

    /// A spawn admitted against a mob lands in that mob or nowhere: when the
    /// mob is destroyed and another session creates one under the same id
    /// between the call's admission and its action, the spawn fails and the
    /// new mob gains no member.
    #[tokio::test]
    async fn an_admitted_spawn_never_lands_in_a_mob_recreated_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "raced-spawn-mob";
        first.create(mob_id).await;

        let outcome = race_call_against_recreate(
            &state,
            &first,
            &second,
            mob_id,
            "mob_spawn_member",
            json!({ "mob_id": mob_id, "profile": "worker", "member_id": "intruder" }),
        )
        .await;

        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(
            member_ids(&state, mob_id).await,
            vec!["b-member".to_string()]
        );
    }

    /// A destroy admitted against a mob destroys that mob or nothing: the
    /// mob another session created under the id in between stays.
    #[tokio::test]
    async fn an_admitted_destroy_never_destroys_a_mob_recreated_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "raced-destroy-mob";
        first.create(mob_id).await;

        let outcome = race_call_against_recreate(
            &state,
            &first,
            &second,
            mob_id,
            "mob_destroy",
            json!({ "mob_id": mob_id }),
        )
        .await;

        let refused = outcome.expect_err("the destroy is refused");
        assert!(
            matches!(refused, ToolError::AccessDenied { .. }),
            "{refused:?}"
        );
        assert_eq!(
            member_ids(&state, mob_id).await,
            vec!["b-member".to_string()]
        );
    }

    /// A keyed member send admitted against a mob commits for that mob's
    /// member or nowhere. When the mob is destroyed and created again under
    /// the id, with a member of the same identity, between the send's
    /// admission and its commit, the send answers that the member it was
    /// admitted for is gone: the key stays uncommitted for the new mob's
    /// member, and nothing reaches the inbox.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn an_admitted_keyed_member_send_never_commits_for_a_mob_recreated_under_its_id() {
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let state = MobMcpState::from_acquired_runtime_adapter(
            Arc::new(crate::LocalSessionService::new_over_runtime_for_tests(
                Arc::clone(&runtime),
            )),
            Some(runtime),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .into_shared();
        let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        ));
        state
            .bind_continuations(inbox.clone(), &meerkat::ContinuationHostBindings::default())
            .expect("bind the state's continuations");
        let mob_id = MobId::from("raced-keyed-send-mob");
        let member = AgentIdentity::from("w-1");
        let create_with_member = || async {
            crate::public_mcp::handle_public_tools_call(
                &state,
                "meerkat_mob_create",
                &json!({
                    "definition": {
                        "id": mob_id.as_str(),
                        "profiles": {
                            "worker": { "model": "gpt-5.4", "tools": {"comms": true} }
                        }
                    }
                }),
            )
            .await
            .expect("create the mob");
            crate::public_mcp::handle_public_tools_call(
                &state,
                "meerkat_mob_spawn",
                &json!({
                    "mob_id": mob_id.as_str(),
                    "profile": "worker",
                    "agent_identity": member.as_str(),
                    "runtime_mode": "turn_driven"
                }),
            )
            .await
            .expect("spawn the member");
        };
        create_with_member().await;

        let hold = hold_next_agent_call(&state);
        let send = state.mob_member_send_keyed(
            &mob_id,
            member.clone(),
            ContentInput::from("raced keyed send"),
            HandlingMode::Queue,
            "raced-item",
        );
        let race = async {
            wait_parked(&hold).await;
            state
                .mob_destroy(&mob_id)
                .await
                .expect("the console destroys the admitted mob");
            create_with_member().await;
            hold.release.notify_one();
        };
        let (outcome, ()) = tokio::join!(send, race);

        assert!(
            matches!(&outcome, Err(MobError::MemberNotFound(identity)) if identity == &member),
            "the member the send was admitted for is gone: {outcome:?}"
        );
        let status = state
            .continuation_service()
            .expect("bound")
            .continuation_status(
                &meerkat::ContinuationOwner::Member {
                    mob_id: mob_id.to_string(),
                    identity: member.to_string(),
                },
                &meerkat::ContinuationKey::new(format!(
                    "{MEMBER_SEND_CONTINUATION_KEY_PREFIX}raced-item"
                ))
                .expect("key"),
            )
            .await
            .expect("status");
        assert_eq!(
            status,
            meerkat::ContinuationStatus::NotCommitted,
            "nothing is committed for the new mob's member"
        );
        assert_eq!(
            inbox
                .pending_delivery_total()
                .await
                .expect("read the inbox"),
            0,
            "nothing reaches the inbox"
        );
    }

    /// A member list admitted against a mob never reads the mob another
    /// session created under the id in between.
    #[tokio::test]
    async fn an_admitted_member_list_never_reads_a_mob_recreated_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "raced-list-mob";
        first.create(mob_id).await;

        let outcome = race_call_against_recreate(
            &state,
            &first,
            &second,
            mob_id,
            "mob_list_members",
            json!({ "mob_id": mob_id }),
        )
        .await;

        if let Ok(listed) = outcome {
            assert!(
                !listed.result.text_content().contains("b-member"),
                "the list read the recreated mob: {}",
                listed.result.text_content()
            );
        }
    }

    /// A delegate that found its session's implicit mob acts on it only: when
    /// that mob is destroyed and another session creates a mob under the
    /// implicit id between finding the mob and acting on it, the delegate is
    /// refused with the ordinary access denial and grants nothing.
    #[tokio::test]
    async fn a_delegate_never_acts_on_a_mob_that_took_its_implicit_id_mid_call() {
        let state = MobMcpState::new_in_memory();
        let owner = OperatorSession::new(&state);
        let squatter = OperatorSession::new(&state);
        let (implicit_id, created) = state
            .ensure_implicit_mob_for_model(&owner.session_id.to_string(), "claude-sonnet-4-5", None)
            .await
            .expect("the owner's implicit mob");
        assert!(created);

        let hold = hold_next_agent_call(&state);
        let delegate = owner.call(
            "delegate",
            json!({
                "task": "say hi",
                "member_id": "helper",
                "result_label": "delegate_result",
                "max_text_bytes": 4096
            }),
        );
        let race = async {
            wait_parked(&hold).await;
            state
                .mob_destroy_unchecked(&implicit_id)
                .await
                .expect("the console destroys the implicit mob");
            squatter.create(implicit_id.as_str()).await;
            hold.release.notify_one();
        };
        let (outcome, ()) = tokio::join!(delegate, race);

        let refused = outcome.expect_err("the delegate is refused");
        assert!(
            matches!(refused, ToolError::AccessDenied { .. }),
            "{refused:?}"
        );
        {
            let authority = owner
                .authority
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(authority.managed_mob_scope().is_empty());
        }
        assert!(member_ids(&state, implicit_id.as_str()).await.is_empty());
    }

    /// A create records its operator provenance in the mob it started or
    /// nowhere: when that mob is destroyed and another session creates one
    /// under the id before the record is written, the new mob's log holds its
    /// own creator's provenance only.
    #[tokio::test]
    async fn a_create_never_records_provenance_in_a_mob_recreated_under_its_id() {
        let state = MobMcpState::new_in_memory();
        let first = OperatorSession::new(&state);
        let second = OperatorSession::new(&state);
        let mob_id = "raced-create-mob";

        let hold = hold_next_agent_call(&state);
        let create = first.create(mob_id);
        let race = async {
            wait_parked(&hold).await;
            state
                .mob_destroy(&MobId::from(mob_id))
                .await
                .expect("the console destroys the created mob");
            second.create(mob_id).await;
            hold.release.notify_one();
        };
        tokio::join!(create, race);

        let principal_of = |session: &OperatorSession| {
            session
                .authority
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .principal_token()
                .clone()
        };
        let events = state
            .handle_for(&MobId::from(mob_id))
            .await
            .expect("the mob under the id")
            .poll_events(0, 10_000)
            .await
            .expect("the mob's events");
        let recorded: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.kind {
                meerkat_mob::MobEventKind::OperatorActionRecorded {
                    tool_name,
                    principal_token,
                    ..
                } => Some((tool_name.clone(), principal_token.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            recorded,
            vec![("mob_create".to_string(), principal_of(&second))],
            "only the new mob's creator is recorded in it"
        );
        assert_ne!(principal_of(&first), principal_of(&second));
    }

    /// While a destroyed mob's storage cleanup is owed, its id is not
    /// created again, since a new mob would use that storage; once nothing
    /// is owed the id is free.
    #[tokio::test]
    async fn an_id_whose_destroy_storage_cleanup_is_owed_is_not_created_again() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("owed-cleanup-id"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");
        state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the held reader keeps the storage cleanup outstanding");

        let refused = state
            .mob_create_definition(explicit_definition("owed-cleanup-id"))
            .await
            .expect_err("the id is refused while its cleanup is owed");
        assert!(
            refused
                .to_string()
                .contains("storage cleanup is still outstanding"),
            "{refused}"
        );

        drop(reader);
        state
            .mob_destroy(&mob_id)
            .await
            .expect("the same caller's retry completes the cleanup");
        state
            .mob_create_definition(explicit_definition("owed-cleanup-id"))
            .await
            .expect("the id is free once nothing is owed");
    }

    /// The caller of an admitted destroy is held back after the native
    /// destroy replied, and then cancelled. The destroy's record keeps the
    /// canonical report and its cleanup, which finishes without the caller,
    /// and the same caller's retry answers that report.
    #[tokio::test]
    async fn a_destroy_cancelled_after_the_native_reply_keeps_its_report_and_cleanup() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("cancelled-after-reply"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let (archive, mut actor_task) = spawn_worker_with_held_archive(&svc, &state, &mob_id).await;

        let payload = json!({ "mob_id": mob_id.to_string(), "action": "destroy" });
        let mut caller = Box::pin(crate::public_mcp::handle_public_tools_call(
            &state,
            "meerkat_mob_lifecycle",
            &payload,
        ));
        tokio::time::timeout(Duration::from_secs(30), async {
            tokio::select! {
                biased;
                () = wait_until_reached(&archive) => {}
                outcome = &mut caller => {
                    panic!("the destroy answered before it archived its member: {outcome:?}")
                }
            }
        })
        .await
        .expect("the native destroy archives its member");
        // From here the caller is never polled again: the native destroy
        // replies and its actor exits while the caller's continuation waits,
        // and then the caller is cancelled.
        archive.release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), actor_task.wait())
                .await
                .expect("the actor task ends"),
            meerkat_mob::MobActorTaskWait::Returned
        );
        drop(caller);

        let recorded = recorded_destroy(&state, &mob_id).await;
        assert_eq!(recorded.caller, MobDestroyCaller::Console);
        assert!(
            recorded.report.metadata_scrubbed && recorded.report.events_cleared,
            "{:?}",
            recorded.report
        );
        let canonical = serde_json::to_value(&recorded.report).expect("report json");

        let retried =
            crate::public_mcp::handle_public_tools_call(&state, "meerkat_mob_lifecycle", &payload)
                .await
                .expect("the same caller's retry answers the recorded destroy");
        assert_eq!(retried["ok"], true);
        assert_eq!(retried["destroy_report"], canonical);
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// An admitted self-destroy may end the task that asked for it: here the
    /// requesting agent's task is cancelled while the destroy archives the
    /// member sessions, as a member's own turn is. The destroy still
    /// finishes on the state's task, removes the storage without any retry,
    /// and the same caller collects the canonical report afterwards.
    #[tokio::test]
    async fn an_admitted_destroy_that_ends_its_callers_task_still_finishes() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("self-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let (archive, _actor_task) = spawn_worker_with_held_archive(&svc, &state, &mob_id).await;
        let surface = Arc::new(destroy_surface(
            &state,
            SessionId::new(),
            managing_authority(&mob_id, live_incarnation(&state, &mob_id).await),
        ));

        let requester = tokio::spawn({
            let surface = Arc::clone(&surface);
            let mob_id = mob_id.clone();
            async move { dispatch_destroy(&surface, &mob_id).await.map(|_| ()) }
        });
        tokio::time::timeout(Duration::from_secs(30), wait_until_reached(&archive))
            .await
            .expect("the native destroy archives its member");
        requester.abort();
        assert!(
            requester
                .await
                .expect_err("the requesting task ended")
                .is_cancelled()
        );
        archive.release.notify_one();

        tokio::time::timeout(Duration::from_secs(30), async {
            while !existing_storage_files(&storage_path).is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the destroy removes its storage without its caller");
        // The files are gone before the cleanup attempt ends: it still
        // verifies the removal before it records that nothing is owed, and
        // a running attempt counts as owed.
        let recorded = recorded_destroy(&state, &mob_id).await;
        tokio::time::timeout(Duration::from_secs(30), async {
            while recorded.cleanup_owed() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the destroy records its storage cleanup as settled without its caller");

        let collected = dispatch_destroy(&surface, &mob_id)
            .await
            .expect("the same caller collects the report");
        let body: serde_json::Value =
            serde_json::from_str(&collected.result.text_content()).expect("destroy JSON");
        assert_eq!(body["ok"], true);
        assert_eq!(
            body["destroy_report"],
            serde_json::to_value(&recorded.report).expect("report json")
        );
    }

    /// A mob registered under a mob's id while that mob's destroy runs is
    /// another mob: the destroy removes only the instance it destroyed, the
    /// registered one stays managed, and the destroyed mob's storage cleanup
    /// stays owed instead of running under it. A retry continues that owed
    /// cleanup and never destroys the registered mob, a fresh destroy under
    /// the id never replaces its record, and once the id is free the retry
    /// settles it.
    #[tokio::test]
    async fn a_mob_registered_under_the_id_during_its_destroy_stays_managed() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("replaced-during-destroy"))
            .await
            .expect("create explicit mob");
        let destroyed_incarnation = live_incarnation(&state, &mob_id).await;
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let (archive, _actor_task) = spawn_worker_with_held_archive(&svc, &state, &mob_id).await;
        let elsewhere = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let other_id = elsewhere
            .mob_create_definition(explicit_definition("registered-instead"))
            .await
            .expect("create the other mob");
        let other = elsewhere
            .handle_for(&other_id)
            .await
            .expect("the other mob's handle");

        let destroying = tokio::spawn({
            let state = Arc::clone(&state);
            let mob_id = mob_id.clone();
            async move { state.mob_destroy(&mob_id).await }
        });
        tokio::time::timeout(Duration::from_secs(30), wait_until_reached(&archive))
            .await
            .expect("the native destroy archives its member");
        state
            .try_mob_insert_handle(mob_id.clone(), other)
            .await
            .expect("register another mob under the id");
        archive.release.notify_one();

        let incomplete = destroying
            .await
            .expect("the destroy task joins")
            .expect_err("the cleanup stays owed while another mob holds the id");
        let MobMcpDestroyError::Incomplete { report } = incomplete else {
            panic!("only the storage cleanup may be outstanding: {incomplete:?}");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("managed under this id again")),
            "{report:?}"
        );
        let managed = state
            .handle_for(&mob_id)
            .await
            .expect("the registered mob stays managed");
        assert_eq!(
            managed.definition().id,
            other_id,
            "the registered mob, not the destroyed one"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "the destroyed mob's storage is kept while the id is managed"
        );

        // The destroy's own caller retries while the registered mob holds the
        // id: the retry continues that exact record, and destroys nothing.
        let recorded = recorded_destroy(&state, &mob_id).await;
        let registered_task = managed.actor_task_watch();
        drop(managed);
        let retried = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the retry continues the owed cleanup while another mob holds the id");
        let MobMcpDestroyError::Incomplete { report } = retried else {
            panic!("the retry continues the recorded destroy: {retried:?}");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("managed under this id again")),
            "{report:?}"
        );
        let still_managed = state
            .handle_for(&mob_id)
            .await
            .expect("the registered mob stays managed after the retry");
        assert!(
            registered_task.observes(&still_managed),
            "the same registered mob, not a destroyed or replaced one"
        );
        assert!(
            !still_managed.actor_command_channel_closed_for_test(),
            "the registered mob's actor still runs"
        );
        drop(still_managed);
        assert!(
            tokio::fs::metadata(&storage_path).await.is_ok(),
            "the destroyed mob's storage is still kept"
        );
        let retained = recorded_destroy(&state, &mob_id).await;
        assert!(
            Arc::ptr_eq(&retained, &recorded),
            "the retry keeps the destroy's own record"
        );
        assert_eq!(
            retained
                .storage
                .as_ref()
                .map(|storage| storage.path.as_path()),
            Some(storage_path.as_path())
        );
        assert!(retained.cleanup_owed());
        assert_eq!(retained.incarnation, destroyed_incarnation.incarnation());
        let agent = destroy_surface(
            &state,
            SessionId::new(),
            managing_authority(&mob_id, destroyed_incarnation),
        );
        let refused = dispatch_destroy(&agent, &mob_id)
            .await
            .expect_err("another caller is refused while the cleanup is owed");
        assert!(
            matches!(refused, ToolError::AccessDenied { .. }),
            "{refused:?}"
        );

        // A route that destroys without consulting the record (session
        // cleanup) may destroy the registered mob: that fresh destroy never
        // replaces the owed record.
        state
            .mob_destroy_unchecked(&mob_id)
            .await
            .expect("the cleanup route destroys the registered mob");
        assert!(
            Arc::ptr_eq(&recorded_destroy(&state, &mob_id).await, &recorded),
            "a fresh destroy under the id keeps the owed record"
        );
        assert!(recorded.cleanup_owed());
        // With the id free, the original caller's retry settles the cleanup.
        let settled = state
            .mob_destroy(&mob_id)
            .await
            .expect("the original caller's retry settles the cleanup");
        assert_eq!(
            serde_json::to_value(&settled).expect("report json"),
            serde_json::to_value(&recorded.report).expect("report json")
        );
        assert!(!recorded.cleanup_owed());
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// A denied destroy is typed feedback to its caller: nothing is
    /// destroyed or recorded, and the caller's task is not ended.
    #[tokio::test]
    async fn a_denied_agent_destroy_is_typed_feedback_and_destroys_nothing() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("denied-destroy"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let unscoped = destroy_surface(
            &state,
            SessionId::new(),
            meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
                .expect("generated create-only mob authority"),
        );

        let denied = dispatch_destroy(&unscoped, &mob_id)
            .await
            .expect_err("an agent without the mob's scope is denied");
        assert!(
            matches!(denied, ToolError::AccessDenied { .. }),
            "{denied:?}"
        );
        assert!(state.handle_for(&mob_id).await.is_ok(), "the mob is kept");
        assert!(!state.mob_is_retired(&mob_id).await);
        assert!(
            !state
                .retired_mobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&mob_id),
            "a denied destroy records nothing"
        );
        assert!(tokio::fs::metadata(&storage_path).await.is_ok());
    }

    /// A file that took the destroyed database's path while its cleanup was
    /// owed is never removed: the retry reports it and leaves it in place.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_file_that_took_the_destroyed_database_path_is_never_removed() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("replaced-database"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");
        state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the held reader keeps the storage cleanup outstanding");
        drop(reader);
        // Dropping the reader only asks its observer's watch worker to stop; a
        // catch-up read still in flight reopens the WAL. Settle it the way the
        // destroy's own retry does, before the replacement takes the path.
        let path = storage_path.clone();
        MobMcpState::observer_drain_settled(
            tokio::task::spawn_blocking(move || {
                meerkat_mob::SqliteMobStores::drain_observers(&path)
            })
            .await
            .expect("observer drain thread"),
        )
        .expect("nothing of the dropped reader's observer keeps running");
        // Renamed over the path, so the replacement never shares the
        // destroyed database's inode.
        let staged = storage_path.with_extension("replacement");
        std::fs::write(&staged, b"another database").expect("stage the replacement");
        std::fs::rename(&staged, &storage_path).expect("replace the database");

        let incomplete = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("a replaced database keeps the cleanup outstanding");
        let MobMcpDestroyError::Incomplete { report } = incomplete else {
            panic!("only the storage cleanup may be outstanding: {incomplete:?}");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("other than the destroyed mob's database")),
            "{report:?}"
        );
        assert_eq!(
            std::fs::read(&storage_path).expect("the replacement is kept"),
            b"another database"
        );
    }

    /// A removal that fails part way (the WAL sidecar is a directory) keeps
    /// the cleanup owed; the database is gone, and once the obstacle is gone
    /// the same caller's retry removes the rest and answers the canonical
    /// report.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_partial_storage_removal_is_finished_by_the_same_callers_retry() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");
        let state = persistent_destroy_state(&svc, root.path());
        let mob_id = state
            .mob_create_definition(explicit_definition("partial-removal"))
            .await
            .expect("create explicit mob");
        let storage_path = destroy_test_storage_path(root.path(), &mob_id);
        let reader = MobStorage::persistent(&storage_path).expect("a public reader");
        state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the held reader keeps the storage cleanup outstanding");
        drop(reader);
        // Dropping the reader only asks its observer's watch worker to stop; a
        // catch-up read still in flight reopens the WAL. Settle it the way the
        // destroy's own retry does, before planting the obstacle.
        let path = storage_path.clone();
        MobMcpState::observer_drain_settled(
            tokio::task::spawn_blocking(move || {
                meerkat_mob::SqliteMobStores::drain_observers(&path)
            })
            .await
            .expect("observer drain thread"),
        )
        .expect("nothing of the dropped reader's observer keeps running");
        let [_, wal, _] = MobMcpState::storage_file_paths(&storage_path);
        if wal.exists() {
            std::fs::remove_file(&wal).expect("clear the WAL sidecar");
        }
        std::fs::create_dir(&wal).expect("a directory where the WAL sidecar goes");

        let incomplete = state
            .mob_destroy(&mob_id)
            .await
            .expect_err("the obstacle keeps the cleanup outstanding");
        let MobMcpDestroyError::Incomplete { report } = incomplete else {
            panic!("only the storage cleanup may be outstanding: {incomplete:?}");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("failed to remove mob storage file")),
            "{report:?}"
        );
        assert!(
            tokio::fs::metadata(&storage_path).await.is_err(),
            "the database itself was removed"
        );
        assert!(wal.is_dir());

        std::fs::remove_dir(&wal).expect("remove the obstacle");
        let report = state
            .mob_destroy(&mob_id)
            .await
            .expect("the same caller's retry finishes the removal");
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(existing_storage_files(&storage_path).is_empty());
    }

    /// Storage is released only by a drain that left nothing of the
    /// database's observer in this process: none was live, or it closed with
    /// its catch-up settled, every worker joined and no replacement
    /// attached. A refusal or anything left open keeps the cleanup owed; a
    /// retained notification registration, which never touches the
    /// database, does not.
    #[test]
    fn only_a_settled_observer_drain_releases_destroyed_storage() {
        use meerkat_mob::{
            SqliteMobObserverDrain as Drain, SqliteMobObserverDrainRefused as Refused,
            SqliteMobObserverDrainReport as Report,
        };
        let settled = Report {
            catch_up_settled: true,
            joined_workers: 1,
            panicked_workers: 0,
            unjoined_workers: 0,
            joining_elsewhere: 0,
            replacement_registered: false,
            retained_registrations: 0,
        };
        assert!(MobMcpState::observer_drain_settled(Ok(Drain::NotFound)).is_ok());
        assert!(MobMcpState::observer_drain_settled(Ok(Drain::Closed(settled))).is_ok());
        assert!(
            MobMcpState::observer_drain_settled(Ok(Drain::Closed(Report {
                retained_registrations: 1,
                ..settled
            })))
            .is_ok()
        );
        for open in [
            Report {
                replacement_registered: true,
                ..settled
            },
            Report {
                catch_up_settled: false,
                ..settled
            },
            Report {
                unjoined_workers: 1,
                ..settled
            },
            Report {
                joining_elsewhere: 1,
                ..settled
            },
        ] {
            assert!(
                MobMcpState::observer_drain_settled(Ok(Drain::Closed(open))).is_err(),
                "{open:?}"
            );
        }
        for refused in [
            Refused::OtherOwners { owner_handles: 1 },
            Refused::UnresolvedPath {
                kind: std::io::ErrorKind::NotFound,
            },
        ] {
            assert!(MobMcpState::observer_drain_settled(Err(refused)).is_err());
        }
    }

    #[tokio::test]
    async fn test_default_constructor_exposes_realm_profile_crud_with_in_memory_store() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let created = state
            .realm_profile_create("worker", &sample_realm_profile("claude-sonnet-4-6"))
            .await
            .expect("default constructor should provide realm profile store");
        assert_eq!(created.name, "worker");

        let fetched = state
            .realm_profile_get("worker")
            .await
            .expect("get realm profile")
            .expect("stored profile");
        assert_eq!(fetched.profile.model, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn realm_profile_crud_is_owner_only_before_store_access() {
        let principal =
            meerkat_core::auth::PrincipalId::new("realm-profile-viewer").expect("principal id");
        let state = MobMcpState::new_in_memory_as(MobControlPrincipal::External(principal));
        let profile = sample_realm_profile("claude-sonnet-4-6");

        let create = state
            .realm_profile_create("worker", &profile)
            .await
            .expect_err("realm-wide profile mutation requires Owner");
        assert_required_scope(&create, ControlScope::SendCommand);
        let get = state
            .realm_profile_get("worker")
            .await
            .expect_err("realm-wide profile read requires Owner");
        assert_required_scope(&get, ControlScope::List);
        let list = state
            .realm_profile_list()
            .await
            .expect_err("realm-wide profile enumeration requires Owner");
        assert_required_scope(&list, ControlScope::List);
        let update = state
            .realm_profile_update("worker", &profile, 1)
            .await
            .expect_err("realm-wide profile update requires Owner");
        assert_required_scope(&update, ControlScope::SendCommand);
        let delete = state
            .realm_profile_delete("worker", 1)
            .await
            .expect_err("realm-wide profile delete requires Owner");
        assert_required_scope(&delete, ControlScope::SendCommand);
    }

    #[tokio::test]
    async fn test_persistent_root_upgrades_default_realm_profile_store_to_durable_sqlite() {
        let svc = Arc::new(MockSessionSvc::new());
        let root = tempfile::tempdir().expect("tempdir");

        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        state
            .realm_profile_create("worker", &sample_realm_profile("claude-opus-4-8"))
            .await
            .expect("create persistent realm profile");

        let restored = Arc::new(
            MobMcpState::new(svc, meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority")
                .with_persistent_storage_root(Some(root.path().to_path_buf())),
        );
        let fetched = restored
            .realm_profile_get("worker")
            .await
            .expect("get restored realm profile")
            .expect("restored profile should exist");
        assert_eq!(fetched.profile.model, "claude-opus-4-8");
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_cleans_up() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let _mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_some()
        );

        state.destroy_bridge_session_mobs(&sid).await.unwrap();
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_fails_closed_on_incomplete_destroy() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let sid = SessionId::new().to_string();

        let definition = explicit_definition("bridge-session-partial-destroy");
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                definition,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-session-scoped mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let bridge_session_id = state
            .handle_for(&mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced bridge-session cleanup archive failure",
        )
        .await;

        let err = state
            .destroy_bridge_session_mobs(&sid)
            .await
            .expect_err("partial bridge-session cleanup must fail closed");
        assert!(
            matches!(err, MobMcpDestroyError::Incomplete { .. }),
            "expected typed incomplete cleanup error, got {err:?}"
        );
        assert!(
            state.handle_for(&mob_id).await.is_ok(),
            "incomplete bridge-session cleanup must retain the mob retry anchor"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        state
            .destroy_bridge_session_mobs(&sid)
            .await
            .expect("retry should clean bridge-session mob");
        assert!(
            state.handle_for(&mob_id).await.is_err(),
            "successful retry should remove bridge-session mob"
        );
    }

    #[tokio::test]
    async fn test_archive_session_with_mob_cleanup_surfaces_incomplete_and_retries_success() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let owner_session_id = SessionId::new();
        svc.insert_persisted_session(Session::with_id(owner_session_id.clone()))
            .await;

        let definition = explicit_definition("archive-helper-partial-destroy");
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                definition,
                owner_session_id.clone(),
                true,
                false,
            )
            .await
            .expect("create archive-helper-owned mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let bridge_session_id = state
            .handle_for(&mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(
            bridge_session_id.clone(),
            "forced archive-helper cleanup archive failure",
        )
        .await;

        let err = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &owner_session_id,
        )
        .await
        .expect_err("archive helper must fail closed on incomplete mob cleanup");
        let SessionError::FailedWithData { data, .. } = err else {
            panic!("expected typed incomplete session error, got {err:?}");
        };
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(
            state.handle_for(&mob_id).await.is_ok(),
            "incomplete archive helper cleanup must retain the mob retry anchor"
        );
        assert!(
            !svc.session_exists(&owner_session_id).await,
            "first archive attempt should have removed the owner session before mob cleanup failed"
        );
        assert!(
            svc.session_exists(&bridge_session_id).await,
            "failed member archive must retain the bridge session for retry"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &owner_session_id,
        )
        .await
        .expect("retry should report success after retained mob cleanup completes");
        assert!(
            state.handle_for(&mob_id).await.is_err(),
            "successful archive helper retry should remove the mob retry anchor"
        );
        assert!(
            !svc.session_exists(&bridge_session_id).await,
            "successful archive helper retry must archive the worker bridge session"
        );
    }

    /// Archiving a member's session retries the member's retirement. A
    /// retirement the first attempt left stuck is re-driven once per archive
    /// request; while its cause still fails, the archive surfaces the typed
    /// stuck retirement with that cause (never masked, never looped), and once
    /// the cause clears the next archive completes it.
    #[tokio::test]
    async fn test_archive_of_a_member_session_redrives_its_stuck_retirement_and_surfaces_a_still_failing_cause()
     {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let mob_id = state
            .mob_create_definition(explicit_definition("archive-redrive"))
            .await
            .expect("create mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn worker");
        let bridge_session_id = state
            .handle_for(&mob_id)
            .await
            .expect("mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
            .await
            .expect("worker bridge session");
        svc.fail_archive(bridge_session_id.clone(), "forced member archive failure")
            .await;

        let first = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &bridge_session_id,
        )
        .await
        .expect_err("the member archive fails");
        assert!(
            first.to_string().contains("forced member archive failure"),
            "the first archive surfaces the failure: {first}"
        );

        // The cause still fails: the archive re-drives the stuck retirement
        // once and surfaces it, typed, with its cause.
        let still_failing = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &bridge_session_id,
        )
        .await
        .expect_err("a re-driven retirement whose cause still fails surfaces it");
        let text = still_failing.to_string();
        assert!(
            text.contains("is stuck at") && text.contains("forced member archive failure"),
            "the archive surfaces the stuck retirement and its cause: {text}"
        );
        assert!(
            svc.session_exists(&bridge_session_id).await,
            "a failed re-drive keeps the member session for the next retry"
        );

        svc.clear_archive_failure(&bridge_session_id).await;
        crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &bridge_session_id,
        )
        .await
        .expect("the archive re-drives the stuck retirement to completion");
        assert!(
            !svc.session_exists(&bridge_session_id).await,
            "the re-driven retirement archives the member session"
        );
    }

    #[tokio::test]
    async fn test_archive_session_with_mob_cleanup_runs_member_retire_then_child_cleanup() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );
        let parent_mob_id = state
            .mob_create_definition(explicit_definition("archive-helper-live-parent"))
            .await
            .expect("create parent mob");
        let parent_identity = AgentIdentity::from("worker-1");
        state
            .mob_spawn(
                &parent_mob_id,
                ProfileName::from("worker"),
                parent_identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn parent worker");
        let member_session_id = state
            .handle_for(&parent_mob_id)
            .await
            .expect("parent mob handle")
            .resolve_bridge_session_id(&parent_identity)
            .await
            .expect("parent member bridge session");

        let child_definition = explicit_definition("archive-helper-live-member-child");
        let child_mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                child_definition,
                member_session_id.clone(),
                true,
                false,
            )
            .await
            .expect("create child mob owned by parent member session");
        let child_identity = AgentIdentity::from("child-worker-1");
        state
            .mob_spawn(
                &child_mob_id,
                ProfileName::from("worker"),
                child_identity.clone(),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn child worker");
        let child_bridge_session_id = state
            .handle_for(&child_mob_id)
            .await
            .expect("child mob handle")
            .resolve_bridge_session_id(&child_identity)
            .await
            .expect("child worker bridge session");
        svc.fail_archive(
            child_bridge_session_id.clone(),
            "forced archive-helper live-member child cleanup failure",
        )
        .await;

        let err = crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &member_session_id,
        )
        .await
        .expect_err("archive helper must fail closed on child cleanup after member retire");
        let SessionError::FailedWithData { data, .. } = err else {
            panic!("expected typed incomplete child cleanup error, got {err:?}");
        };
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert!(
            !svc.session_exists(&member_session_id).await,
            "successful parent retire must archive the mob member bridge session before child cleanup"
        );
        assert!(
            state.handle_for(&child_mob_id).await.is_ok(),
            "incomplete child cleanup must retain the child mob retry anchor"
        );

        svc.clear_archive_failure(&child_bridge_session_id).await;
        crate::agent_tools::archive_session_with_mob_cleanup(
            svc.clone(),
            state.clone(),
            &member_session_id,
        )
        .await
        .expect("retry should complete retained child cleanup after parent member retire");
        assert!(
            state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child mob retry anchor"
        );
        assert!(
            !svc.session_exists(&child_bridge_session_id).await,
            "successful retry must archive the child worker bridge session"
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_uses_generated_cleanup_authority() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        // Mob ids are unique per test: a mob's supervisor registers
        // `<mob_id>/__mob_supervisor__` in the process-global in-process
        // comms registry, so two concurrently running tests that reuse one id
        // collide with `ParticipantNameOccupied`.
        let manual = explicit_definition("manual-owner-index-destroy");
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual unowned mob");

        let bridge_session_scoped = explicit_definition("bridge-session-scoped-owner");
        let bridge_session_scoped_id = state
            .mob_create_definition_with_owner_bridge_session(
                bridge_session_scoped,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-session-scoped mob");

        state.destroy_bridge_session_mobs(&sid).await.unwrap();

        assert!(
            state.handle_for(&manual_id).await.is_ok(),
            "manual unowned mob must not be eligible for owner-session cleanup"
        );
        assert!(
            state.handle_for(&bridge_session_scoped_id).await.is_err(),
            "generated DestroyOnOwnerArchive authority must remain the cleanup truth"
        );
    }

    #[tokio::test]
    async fn test_destroy_bridge_session_mobs_noop_when_none() {
        let state = MobMcpState::new_in_memory();
        // Should succeed even when no implicit mob exists
        state
            .destroy_bridge_session_mobs("nonexistent")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_get_or_create_produces_single_mob() {
        let state = Arc::new(MobMcpState::new_in_memory());
        let sid = SessionId::new().to_string();
        let n = 20;

        let mut handles = Vec::new();
        for _ in 0..n {
            let s = state.clone();
            let sid = sid.clone();
            handles.push(tokio::spawn(async move {
                s.get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
                    .await
                    .unwrap()
            }));
        }

        let mut mob_ids = Vec::new();
        for h in handles {
            mob_ids.push(h.await.unwrap());
        }

        // All must return the same mob_id
        let first = &mob_ids[0];
        for id in &mob_ids {
            assert_eq!(id, first, "concurrent calls must return same mob_id");
        }

        // Only one mob should exist in the registry
        let mobs = state.mob_list().await.expect("list implicit mobs");
        let implicit_count = mobs.len();
        assert_eq!(implicit_count, 1, "only one implicit mob should exist");
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        // Create a session and its implicit mob
        let result = svc
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .unwrap();
        let sid = result.session_id.to_string();
        let mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();

        // Also create an unowned explicit mob that should survive scavenging.
        // Its id is unique to this test (see
        // `test_destroy_bridge_session_mobs_uses_generated_cleanup_authority`).
        let manual = explicit_definition("manual-owner-index-scavenge");
        let manual_id = state
            .mob_create_definition(manual)
            .await
            .expect("create manual unowned mob");

        // Session exists — scavenge should find nothing.
        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert!(scavenged.is_empty(), "no orphans while session exists");

        // Archive the session — now the mob is orphaned
        svc.archive(&result.session_id).await.unwrap();

        // Scavenge should find and destroy the bridge-session-scoped orphan, but not
        // the manual owner-indexed mob.
        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert_eq!(scavenged, vec![mob_id.clone()]);

        // Mob should be gone
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&sid)
                .await
                .is_none()
        );
        assert!(
            state.handle_for(&manual_id).await.is_ok(),
            "manual unowned mob must survive orphan scavenging"
        );
    }

    #[tokio::test]
    async fn test_scavenge_orphaned_bridge_session_scoped_mobs_honors_bridge_owner_index() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(
            MobMcpState::new(svc.clone(), meerkat_mob::MobControlPrincipal::Owner)
                .expect("construct runtime authority"),
        );

        let result = svc
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: ContentInput::from("test"),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .unwrap();
        let sid = result.session_id.to_string();

        let bridge_owned = explicit_definition("bridge-owned-orphan");
        let bridge_owned_id = state
            .mob_create_definition_with_owner_bridge_session(
                bridge_owned,
                SessionId::parse(&sid).expect("session id"),
                true,
                false,
            )
            .await
            .expect("create bridge-owned bridge-session-scoped mob");

        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert!(
            scavenged.is_empty(),
            "session exists, so bridge-owned mob must not scavenge"
        );

        svc.archive(&result.session_id).await.unwrap();

        let scavenged = state.scavenge_orphaned_bridge_session_scoped_mobs().await;
        assert_eq!(scavenged, vec![bridge_owned_id.clone()]);
        assert!(
            state.handle_for(&bridge_owned_id).await.is_err(),
            "bridge-owned bridge-session-scoped mob must be scavenged once the bridge session disappears"
        );
    }

    #[tokio::test]
    async fn test_implicit_mob_uses_external_owner_wiring() {
        let state = MobMcpState::new_in_memory();
        let sid = SessionId::new().to_string();

        let mob_id = state
            .get_or_create_implicit_mob_for_bridge_session(&sid, "claude-sonnet-4-5")
            .await
            .unwrap();
        let handle = state.handle_for(&mob_id).await.unwrap();
        assert!(
            !handle.definition().wiring.auto_wire_orchestrator,
            "implicit delegate mobs wire the owner as an external peer, not as a local orchestrator"
        );
        assert!(
            handle.definition().orchestrator.is_none(),
            "implicit delegate mobs must not invent a local orchestrator member"
        );
        assert_eq!(
            handle
                .owner_bridge_session_lifecycle_authority()
                .map(|authority| authority.bridge_session_id.to_string()),
            Some(sid),
            "implicit mob must have correct owner_bridge_session_id"
        );
    }

    #[tokio::test]
    async fn test_local_session_service_provides_runtime_adapter() {
        let svc = LocalSessionService::new();
        assert!(
            <LocalSessionService as MobSessionService>::acquire_runtime_adapter(&svc, None)
                .expect("acquire runtime authority")
                .is_some(),
            "LocalSessionService must provide adapter for AutonomousHost"
        );
    }

    #[tokio::test]
    async fn test_local_session_service_apply_runtime_turn_returns_start_turn_terminal_result() {
        let svc = LocalSessionService::new();
        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "test".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        let created = <LocalSessionService as SessionService>::create_session(&svc, req)
            .await
            .expect("create session");
        let result = <LocalSessionService as MobSessionService>::apply_runtime_turn(
            &svc,
            &created.session_id,
            meerkat_core::RunId::new(),
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: "test".into(),
                system_prompt: None,
                event_tx: None,
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
            },
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
            vec![],
        )
        .await;
        assert!(
            result.is_ok(),
            "apply_runtime_turn must work for AutonomousHost: {result:?}"
        );
        let output = result.expect("apply_runtime_turn");
        match output.terminal {
            Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(
                run_result,
            )) => {
                assert_eq!(run_result.text, "ok");
                assert_eq!(run_result.session_id, created.session_id);
            }
            other => panic!("expected start_turn terminal run result, got {other:?}"),
        }
    }

    /// T-B5 (§17.4, DEC-P7B-9): typed MCP constructors — ScopeDenied
    /// carries -32025 + the bare typed `{required, presented}` data,
    /// StaleCursor carries the watermark, and an unmapped `MobError` is
    /// byte-identical to the legacy `invalid_params` fallback.
    #[test]
    fn mcp_tool_error_from_mob_typed_and_fallback() {
        let denied = MobError::ScopeDenied(ScopeDenial {
            required: ControlScope::AdminHost,
            presented: std::collections::BTreeSet::from([ControlScope::List]),
        });
        let err = McpToolError::from_mob(&denied);
        assert_eq!(err.code, -32025, "ScopeDenied carries the stable code");
        assert_eq!(err.message, denied.to_string());
        assert_eq!(
            err.data,
            Some(json!({ "required": "admin_host", "presented": ["list"] })),
            "data is the BARE phase-5 detail shape"
        );

        let stale = MobError::StaleEventCursor {
            after_cursor: 12,
            latest_cursor: 5,
        };
        let err = McpToolError::from_mob(&stale);
        assert_eq!(err.code, -32027, "StaleCursor carries the stable code");
        let data = err.data.expect("stale cursor data");
        assert_eq!(data["watermark"], 5, "reply carries the current watermark");
        assert_eq!(data["requested"], 12);

        let pending = MobError::RetirementInProgress {
            session_id: SessionId::new(),
            stage: "session_archive_and_unregister".to_string(),
        };
        let err = McpToolError::from_mob(&pending);
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code()
        );
        let data = err.data.expect("typed retirement retry data");
        assert_eq!(data["kind"], "mob_retirement_in_progress");
        assert_eq!(data["retryable"], true);
        assert_eq!(data["authority_retained"], true);

        let peer_pending = MobError::MemberRetirementInProgress {
            member_id: AgentIdentity::from("peer-worker"),
            stage: "peer_retirement_reply".to_string(),
        };
        let err = McpToolError::from_mob(&peer_pending);
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code()
        );
        let data = err.data.expect("typed peer retirement retry data");
        assert_eq!(data["member_id"], "peer-worker");
        assert_eq!(data["stage"], "peer_retirement_reply");
        assert_eq!(data["retryable"], true);
        assert_eq!(data["authority_retained"], true);

        let unmapped = MobError::MobNotFound(MobId::from("missing"));
        let err = McpToolError::from_mob(&unmapped);
        let legacy = McpToolError::invalid_params(unmapped.to_string());
        assert_eq!(err.code, legacy.code, "fallback keeps -32602");
        assert_eq!(
            err.message, legacy.message,
            "fallback message is byte-identical"
        );
        assert_eq!(
            err.data, None,
            "fallback carries no data, exactly like legacy"
        );

        let append_denied = MobAppendSystemContextError::Mob(MobError::ScopeDenied(ScopeDenial {
            required: ControlScope::SendCommand,
            presented: BTreeSet::new(),
        }));
        let err = McpToolError::from_mob_append_system_context(&append_denied);
        assert_eq!(err.code, -32025);
        assert_eq!(
            err.data,
            Some(json!({ "required": "send_command", "presented": [] }))
        );

        let append_session =
            MobAppendSystemContextError::Session(SessionControlError::InvalidRequest {
                message: "bad append".to_string(),
            });
        let err = McpToolError::from_mob_append_system_context(&append_session);
        assert_eq!(err.code, -32602);
        assert!(err.data.is_none());
    }

    /// T-B3 (destroy half): `MobMcpDestroyError::wire_detail` delegates for
    /// `Mob(inner)`; the `Incomplete` arm keeps its dedicated
    /// `destroy_incomplete` envelope through `from_mob_destroy`.
    #[test]
    fn destroy_wrapper_delegates_wire_detail_and_keeps_incomplete_envelope() {
        let delegated = MobMcpDestroyError::Mob(MobError::BridgeRequestTimedOut {
            request_envelope_id: "env-3".to_string(),
            timeout_ms: 30_000,
        });
        assert!(
            matches!(
                delegated.wire_detail(),
                Some(meerkat_contracts::wire::WireMobErrorDetail::HostUnavailable(_))
            ),
            "Mob(inner) must delegate the console projection"
        );
        let err = McpToolError::from_mob_destroy(&delegated);
        assert_eq!(err.code, -32026);

        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker: archive failed".to_string());
        let incomplete = MobMcpDestroyError::Incomplete { report };
        assert!(
            incomplete.wire_detail().is_none(),
            "Incomplete keeps its dedicated envelope, not a console code"
        );
        let err = McpToolError::from_mob_destroy(&incomplete);
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        );
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete"),
            "destroy_incomplete data envelope preserved verbatim"
        );
    }

    /// T-B8 (local half; the placed-member half rides the cross-host
    /// fixtures in meerkat-mob): the ONE envelope-assembly point emits
    /// `placement: None` + `ControllingHostVerified` for a locally-served
    /// page, with the shared page body shape.
    #[tokio::test]
    async fn member_history_result_provenance_and_placement_local() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(explicit_definition("history-provenance-mob"))
            .await
            .expect("create mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn local member");

        let result = state
            .mob_member_history(&mob_id, AgentIdentity::from("worker-1"), None, None)
            .await
            .expect("local member history");
        assert_eq!(
            result.placement, None,
            "a locally-served page carries no placement"
        );
        assert_eq!(
            result.provenance,
            meerkat_contracts::wire::WireProjectionProvenance::ControllingHostVerified,
            "controlling-host pages are ControllingHostVerified"
        );
        // Shared page-body invariants (WireMemberHistoryPageBody::try_from_history_page).
        assert_eq!(result.page.from_index, 0);
        assert_eq!(
            result.page.messages.len() as u64,
            result.page.message_count,
            "a complete single page serves the whole transcript"
        );
        assert!(result.page.complete);
        assert_eq!(result.page.next_index, None);
    }

    /// T-B13: scope matrix over the phase-7 wrappers. A List-only named
    /// principal reads the two chokepoint-(b) projections but the
    /// AdminHost-gated revoke and the ReadHistory-gated member history
    /// deny with the typed `{required, presented}` pair (actor-gated).
    #[tokio::test]
    async fn phase7_wrapper_scope_matrix() {
        use meerkat_contracts::wire::{WireControlScope, WireMobErrorDetail};

        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("phase7-scope-matrix"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn member");
        let viewer_id = meerkat_core::auth::PrincipalId::new("viewer").expect("principal id");
        owner
            .mob_grant_scopes(
                &mob_id,
                viewer_id.clone(),
                std::collections::BTreeSet::from([ControlScope::List]),
                None,
            )
            .await
            .expect("owner grants List");

        // The named-principal console shares the same live handle
        // (ADJ-P5-10 deterministic lane); handle_for rebinds it to the
        // viewer principal at chokepoint (b).
        let viewer =
            MobMcpState::new_in_memory_as(MobControlPrincipal::External(viewer_id.clone()));
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        // List admits the two watch-read projections.
        viewer
            .mob_route_installs(&mob_id)
            .await
            .expect("List admits the route-install projection");
        viewer
            .mob_hosts(&mob_id)
            .await
            .expect("List admits the hosts projection");

        // AdminHost-gated host ceremony denies with the typed pair.
        let denied = viewer
            .mob_revoke_host(&mob_id, "host-peer-1")
            .await
            .expect_err("List-only principal cannot drive host ceremony");
        match denied.wire_detail() {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::AdminHost);
                assert_eq!(detail.presented, vec![WireControlScope::List]);
            }
            other => panic!("expected typed AdminHost denial, got {other:?}"),
        }

        // ReadHistory gate on member_history, end-to-end (actor-gated).
        let denied = viewer
            .mob_member_history(&mob_id, AgentIdentity::from("worker-1"), None, None)
            .await
            .expect_err("List-only principal cannot read history");
        match denied.wire_detail() {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::ReadHistory);
                assert_eq!(detail.presented, vec![WireControlScope::List]);
            }
            other => panic!("expected typed ReadHistory denial, got {other:?}"),
        }
    }

    fn assert_required_scope(error: &MobError, required: ControlScope) {
        match error {
            MobError::ScopeDenied(denial) => {
                assert_eq!(denial.required, required);
                assert!(denial.presented.is_empty());
            }
            other => panic!("expected ScopeDenied({required:?}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mob_retire_denies_ungranted_principal_before_member_disposal() {
        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("retire-scope-gate"))
            .await
            .expect("create mob");
        let identity = AgentIdentity::from("retained-worker");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                identity.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn retained member fixture");

        let viewer = MobMcpState::new_in_memory_as(MobControlPrincipal::External(
            meerkat_core::auth::PrincipalId::new("retire-viewer").expect("principal"),
        ));
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        assert_required_scope(
            &viewer
                .mob_retire(&mob_id, identity.clone())
                .await
                .expect_err("ungranted principal must not retire member"),
            ControlScope::Retire,
        );
        owner
            .mob_member_status(&mob_id, &identity)
            .await
            .expect("scope denial must retain the member");
    }

    #[tokio::test]
    async fn composite_surfaces_deny_before_raw_projection_or_fallback() {
        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("composite-scope-precedence"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("existing"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn retained member fixture");

        let viewer = MobMcpState::new_in_memory_as(MobControlPrincipal::External(
            meerkat_core::auth::PrincipalId::new("composite-viewer").expect("principal"),
        ));
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        let append = viewer
            .mob_append_system_context(
                &mob_id,
                &AgentIdentity::from("missing"),
                AppendSystemContextRequest {
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text("x"),
                    source: None,
                    idempotency_key: None,
                },
            )
            .await
            .expect_err("authorization precedes member-session lookup");
        match append {
            MobAppendSystemContextError::Mob(error) => {
                assert_required_scope(&error, ControlScope::SendCommand);
            }
            other => panic!("expected mob-family denial, got {other:?}"),
        }

        assert_required_scope(
            &viewer
                .mob_events_strict(&mob_id, u64::MAX, 1)
                .await
                .expect_err("authorization precedes strict cursor validation"),
            ControlScope::SubscribeEvents,
        );
        assert_required_scope(
            &viewer
                .mob_ensure_member(&mob_id, SpawnMemberSpec::new("worker", "existing"))
                .await
                .expect_err("authorization precedes retain projection"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_reconcile(
                    &mob_id,
                    vec![SpawnMemberSpec::new("worker", "existing")],
                    meerkat_mob::runtime::ReconcileOptions::default(),
                )
                .await
                .expect_err("authorization precedes no-op reconcile"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_list_members_matching(&mob_id, meerkat_mob::runtime::MemberFilter::default())
                .await
                .expect_err("authorization precedes watch filtering"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_list_flows(&mob_id)
                .await
                .expect_err("authorization precedes definition projection"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_spawn_many(&mob_id, Vec::new())
                .await
                .expect_err("empty batch still authorizes"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_member_send(
                    &mob_id,
                    AgentIdentity::from("missing"),
                    ContentInput::from("hello"),
                    HandlingMode::Queue,
                    None,
                )
                .await
                .expect_err("authorization precedes missing-member lookup"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_ingress_interaction(
                    &mob_id,
                    SpawnMemberSpec::new("worker", "ingress-member"),
                    ContentInput::from("hello"),
                    HandlingMode::Queue,
                    None,
                )
                .await
                .expect_err("authorization precedes ingress cursor and ensure projections"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_turn_start_target(&mob_id, &AgentIdentity::from("missing"))
                .await
                .expect_err("authorization precedes turn target resolution"),
            ControlScope::SendCommand,
        );
        assert_required_scope(
            &viewer
                .mob_member_status(&mob_id, &AgentIdentity::from("existing"))
                .await
                .expect_err("authorization precedes member-status shortcuts"),
            ControlScope::List,
        );
        assert_required_scope(
            &viewer
                .mob_list_runs(&mob_id, None)
                .await
                .expect_err("authorization precedes run-store projection"),
            ControlScope::List,
        );
        let helper_error = viewer
            .mob_spawn_helper(
                &mob_id,
                AgentIdentity::from("denied-helper"),
                "should not spawn".to_string(),
                meerkat_mob::HelperOptions::default(),
                "denied_helper_result",
                meerkat_mob::DEFAULT_BOUNDED_HELPER_RESULT_BYTES,
            )
            .await
            .expect_err("authorization precedes helper mechanics");
        let meerkat_mob::BoundedMemberRunError::Admission(helper_error) = helper_error else {
            panic!("authorization must reject before helper turn admission")
        };
        assert_required_scope(&helper_error, ControlScope::SendCommand);
        assert_required_scope(
            &viewer
                .mob_cancel_work(&mob_id, meerkat_mob::WorkRef::new())
                .await
                .expect_err("authorization precedes unsupported capability result"),
            ControlScope::Cancel,
        );

        let implicit_id = owner
            .get_or_create_implicit_mob_for_bridge_session(
                &SessionId::new().to_string(),
                "claude-sonnet-4-5",
            )
            .await
            .expect("create implicit mob");
        viewer
            .mob_insert_handle(
                implicit_id.clone(),
                owner
                    .handle_for(&implicit_id)
                    .await
                    .expect("implicit owner handle"),
            )
            .await;
        let destroy = viewer
            .mob_destroy(&implicit_id)
            .await
            .expect_err("authorization precedes implicit-mob guard");
        match destroy {
            MobMcpDestroyError::Mob(error) => assert_required_scope(&error, ControlScope::Retire),
            other => panic!("expected mob-family destroy denial, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn composite_operations_do_not_require_unrelated_read_or_retire_scopes() {
        let owner = MobMcpState::new_in_memory();
        let mob_id = owner
            .mob_create_definition(explicit_definition("composite-scope-grants"))
            .await
            .expect("create mob");
        owner
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                AgentIdentity::from("worker-1"),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn member");
        let principal =
            meerkat_core::auth::PrincipalId::new("composite-operator").expect("principal");
        owner
            .mob_grant_scopes(
                &mob_id,
                principal.clone(),
                BTreeSet::from([
                    ControlScope::SendCommand,
                    ControlScope::SubscribeEvents,
                    ControlScope::Cancel,
                ]),
                None,
            )
            .await
            .expect("grant operation scopes without List or Retire");
        let operator =
            MobMcpState::new_in_memory_as(MobControlPrincipal::External(principal.clone()));
        operator
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;

        operator
            .mob_member_binding_for_send_command(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("write-target resolution must not require List");
        operator
            .mob_member_binding_for_cancel(&mob_id, &AgentIdentity::from("worker-1"))
            .await
            .expect("cancel-target resolution must not require List");
        operator
            .mob_ensure_member(
                &mob_id,
                SpawnMemberSpec::new("worker", "worker-1")
                    .with_runtime_mode(MobRuntimeMode::TurnDriven),
            )
            .await
            .expect("retained ensure requires SendCommand, not List");
        assert!(
            operator
                .mob_spawn_many(&mob_id, Vec::new())
                .await
                .expect("authorized empty batch")
                .is_empty()
        );
        assert!(matches!(
            operator
                .mob_cancel_work(&mob_id, meerkat_mob::WorkRef::new())
                .await,
            Err(MobError::WorkCancellationUnsupported(_))
        ));

        let reconcile_error = operator
            .mob_reconcile(
                &mob_id,
                vec![
                    SpawnMemberSpec::new("worker", "worker-1")
                        .with_runtime_mode(MobRuntimeMode::TurnDriven),
                ],
                meerkat_mob::runtime::ReconcileOptions { retire_stale: true },
            )
            .await
            .expect_err("retire-stale reconcile additionally requires Retire");
        match reconcile_error {
            MobError::ScopeDenied(denial) => {
                assert_eq!(denial.required, ControlScope::Retire);
                assert_eq!(
                    denial.presented,
                    BTreeSet::from([
                        ControlScope::SendCommand,
                        ControlScope::SubscribeEvents,
                        ControlScope::Cancel,
                    ])
                );
            }
            other => panic!("expected Retire denial, got {other:?}"),
        }

        let flow_error = operator
            .mob_run_flow(&mob_id, FlowId::from("missing-flow"), json!({}))
            .await
            .expect_err("fixture flow is absent after SendCommand admission");
        assert!(
            !matches!(flow_error, MobError::ScopeDenied(_)),
            "flow preview must not impose List on a SendCommand principal"
        );

        operator
            .mob_spawn_helper(
                &mob_id,
                AgentIdentity::from("helper-1"),
                "small helper task".to_string(),
                meerkat_mob::HelperOptions::default(),
                "helper_result",
                meerkat_mob::DEFAULT_BOUNDED_HELPER_RESULT_BYTES,
            )
            .await
            .expect("helper mechanics must not require List or Retire after SendCommand admission");

        let empty_wait_mob = owner
            .mob_create_definition(explicit_definition("subscribe-only-wait"))
            .await
            .expect("create empty wait mob");
        owner
            .mob_grant_scopes(
                &empty_wait_mob,
                principal.clone(),
                BTreeSet::from([ControlScope::SubscribeEvents]),
                None,
            )
            .await
            .expect("grant SubscribeEvents");
        operator
            .mob_insert_handle(
                empty_wait_mob.clone(),
                owner
                    .handle_for(&empty_wait_mob)
                    .await
                    .expect("empty wait handle"),
            )
            .await;
        assert!(
            operator
                .mob_wait_ready(&empty_wait_mob, None, Some(10))
                .await
                .expect("SubscribeEvents-only wait must not require List")
                .is_empty()
        );
    }

    /// The collector's job is the member -> session-id -> usage mapping. Each
    /// member's session is primed with distinct numbers, so a transposed
    /// mapping cannot pass: the projection unit tests cannot catch that class
    /// of defect because they are handed the mapping already resolved.
    #[tokio::test]
    async fn run_accounting_attributes_each_session_usage_to_its_own_member() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = MobMcpState::new(svc.clone(), MobControlPrincipal::Owner)
            .expect("construct runtime authority");
        let mob_id = state
            .mob_create_definition(explicit_definition("run-accounting-mapping"))
            .await
            .expect("create mob");

        let mut expected: Vec<(AgentIdentity, SessionId, u64)> = Vec::new();
        for (index, identity) in ["auditor-1", "auditor-2"].into_iter().enumerate() {
            let identity = AgentIdentity::from(identity);
            state
                .mob_spawn(
                    &mob_id,
                    ProfileName::from("worker"),
                    identity.clone(),
                    Some(MobRuntimeMode::TurnDriven),
                    None,
                    None,
                )
                .await
                .expect("spawn accounting fixture member");
            let session_id = state
                .mob_member_status(&mob_id, &identity)
                .await
                .expect("member status")
                .current_session_id
                .expect("a spawned turn-driven member holds a bridge session");
            let input_tokens = 100 * (index as u64 + 1);
            svc.prime_session_usage(
                &session_id,
                Usage {
                    input_tokens,
                    output_tokens: 7,
                    ..Default::default()
                },
            )
            .await;
            expected.push((identity, session_id, input_tokens));
        }

        let accounting = state
            .mob_run_accounting(&mob_id)
            .await
            .expect("roster is readable, so the projection is produced");

        assert_eq!(accounting.members.len(), expected.len());
        assert_eq!(accounting.members_usage_unavailable, 0);
        for (identity, session_id, input_tokens) in &expected {
            let member = accounting
                .members
                .iter()
                .find(|member| member.agent_identity == identity.as_str())
                .expect("every roster member is projected");
            assert_eq!(
                member.session_id.as_deref(),
                Some(session_id.to_string().as_str()),
                "member must carry its own bridge session id"
            );
            let usage = member
                .usage
                .as_ref()
                .expect("a readable session reports usage");
            assert_eq!(
                usage.input_tokens, *input_tokens,
                "usage must be attributed to the member whose session it came from"
            );
            assert!(member.usage_unavailable.is_none());
            assert!(
                accounting
                    .member_session_ids
                    .iter()
                    .any(|pointer| pointer == &session_id.to_string()),
                "the export pointer set must contain every member session"
            );
        }
        assert_eq!(accounting.usage_total.input_tokens, 300);
        assert_eq!(accounting.usage_total.output_tokens, 14);
        assert_eq!(
            accounting.attribution,
            meerkat_contracts::WireMobRunUsageAttribution::SessionCumulative
        );
    }

    /// A member whose session the host cannot read is named, not zeroed, and
    /// the members it can read keep their numbers.
    #[tokio::test]
    async fn run_accounting_degrades_per_member_when_a_session_is_unreadable() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = MobMcpState::new(svc.clone(), MobControlPrincipal::Owner)
            .expect("construct runtime authority");
        let mob_id = state
            .mob_create_definition(explicit_definition("run-accounting-degrade"))
            .await
            .expect("create mob");
        let readable = AgentIdentity::from("readable-1");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                readable.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn readable member");
        let readable_session = state
            .mob_member_status(&mob_id, &readable)
            .await
            .expect("member status")
            .current_session_id
            .expect("bridge session");
        svc.prime_session_usage(
            &readable_session,
            Usage {
                input_tokens: 42,
                output_tokens: 2,
                ..Default::default()
            },
        )
        .await;

        // Drop the session document behind the second member's back so its
        // read fails the way a remote or archived session does.
        let unreadable = AgentIdentity::from("unreadable-1");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("worker"),
                unreadable.clone(),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn second member");
        let unreadable_session = state
            .mob_member_status(&mob_id, &unreadable)
            .await
            .expect("member status")
            .current_session_id
            .expect("bridge session");
        svc.forget_session(&unreadable_session).await;

        let accounting = state
            .mob_run_accounting(&mob_id)
            .await
            .expect("one unreadable member must not fail the whole projection");
        assert_eq!(accounting.members.len(), 2);
        assert_eq!(accounting.members_usage_unavailable, 1);
        assert_eq!(accounting.usage_total.input_tokens, 42);
        let degraded = accounting
            .members
            .iter()
            .find(|member| member.agent_identity == unreadable.as_str())
            .expect("the unreadable member is still projected");
        assert!(degraded.usage.is_none());
        assert!(
            degraded
                .usage_unavailable
                .as_deref()
                .is_some_and(|reason| reason.contains("not readable here")),
            "absence must carry its reason: {degraded:?}"
        );
        // Its session id still rides along: unreadable here is still
        // exportable where the session lives.
        assert_eq!(
            degraded.session_id,
            Some(unreadable_session.to_string()),
            "an unreadable-here session is still exportable where it lives"
        );
    }
}
