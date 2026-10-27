//! Mob announcements: members of a mob, and the host, announce developments
//! to the mob; members subscribe to topics of the mobs they name; the
//! announcements each subscriber is owed reach it in bounded batches,
//! durably and reconcilably.
//!
//! Active only when a host enables it at startup
//! (`runtime_options.mob_announcements`). When it is off nothing here runs.
//!
//! # Ownership
//!
//! This module owns announcements, member subscriptions, capture, batch
//! sealing and its own delivery progress. Meerkat owns everything native: a
//! sealed batch is delivered as one host continuation
//! ([`native::ANNOUNCEMENT_CONTINUATION_NAMESPACE`], keyed by batch id)
//! through Meerkat's continuation owner, which commits it into the member's
//! durable runtime inbox, binds it to the member incarnation current at the
//! first submission, and admits it as one input when the member is served.
//! The announcement owner reads those facts back; it never mirrors or decides
//! them.
//!
//! # Facts a caller can observe
//!
//! For each delivery, `meta/mob_announcements/deliveries/observe` reports
//! separately: the announcements it carries, the sealed batch (what the
//! member is owed), the native commit receipt, native admission as an input,
//! that input's own runtime state, and its handling: what became of the
//! turn, read from the input's own receipt (see [`handling`]). There is no
//! single "processed" flag: admission says nothing about what the agent
//! did, and a turn that ended before completing is reported as such.
//!
//! # Attention policy
//!
//! An announcement is captured for every matching subscription in the
//! transaction that commits it. A member's owed announcements are sealed
//! into at most one outstanding batch at a time, at most one batch per wake
//! interval, when the batch is full, the oldest owed announcement has waited
//! `max_delay_ms`, or a paused subscription resumes. No model call happens
//! per announcement.

pub mod availability;
pub mod declarations;
pub mod escalations;
pub(crate) mod faults;
pub mod handling;
pub(crate) mod host_store;
pub mod native;
pub mod owner;
pub mod remote;
pub mod rpc;
pub mod sources;
pub mod sqlite;
pub(crate) mod storage;
pub mod store;
pub mod tools;
pub mod types;

#[cfg(test)]
mod escalations_tests;
#[cfg(test)]
mod host_store_tests;
#[cfg(test)]
mod remote_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use serde::Deserialize;

pub use availability::{
    BlockCause, DepartureEvidence, EvidenceClass, MemberDeparture, NotServedHereCause,
    SuspendCause, SuspendMarker,
};
pub use declarations::{
    DeclarationRecordKind, DeclarationRecordView, DeclarationRefusal, DeclarationStatus,
    Declarations, DeclarationsSource, DeclarationsView, DeclaredCollection, DeclaredDelivery,
    DeclaredSource, DeclaredSubscription, PutRoleTemplate, ROLE_TEMPLATE_LIMIT, RoleSelector,
    RoleTemplate,
};
pub use escalations::{
    Escalation, EscalationCause, EscalationEvent, EscalationEventKind, EscalationSubject,
    HEALTH_TOPIC, MemberBacklogRead, MemberHandlingRead, MemberRead, QuietEvidence,
};
pub use handling::{BatchHandling, HandlingRead, HandlingState, MAX_REDELIVERIES};
pub use native::{
    ANNOUNCEMENT_CONTINUATION_NAMESPACE, AnnouncementNativeDelivery, IdentityService, MemberMobEnd,
    MemberMobScan, MemberProblem, MemberRoute, MobNativeDelivery, MobPresence, NativeDelivery,
    NativeStatus, NativeStatusError, NativeSubmitError, NativeUnavailable, PrepareError,
    batch_summary, continuation_delivery, continuation_for,
};
pub use owner::{
    AdmittedInput, AnnouncementConfig, AnnouncementError, AnnouncementService,
    DEFAULT_RENOTIFY_AFTER_MS, DEFAULT_RETENTION_MS, DeliveryObservation, ErrorSink,
    MIN_RETENTION_MS, NativeObservation, RoundReport, SourceRunners,
};
pub use remote::{KeyedInjectServer, RemoteMemberDelivery};
pub use sources::{
    AdvanceSource, AnnouncementSource, AudienceGrant, AuditAction, AuditQuery, AuditReason,
    AuditRow, BaselineCapability, CollectionPhase, ConnectionKind, Coverage, CoverageGap,
    DeletionCapability, FailureClass, ItemChange, ItemOutcome, ItemOutcomeKind, MemberFacts,
    ObservationCapability, Observed, OnboardingMode, Principal, PutSource, Rebaseline,
    RebaselineReason, RecordSourceFailure, RosterFacts, SourceAdvance, SourceChange, SourceConfig,
    SourceConflictRecord, SourceConnection, SourceCursor, SourceDelivery, SourceFailure,
    SourceFailureRecord, SourceHealth, SourceHealthPolicy, SourceHealthStatus, SourceIdentity,
    SourceItem, SourcePage, SourcePriority, SourceQuery, SourceRoute, SourceRunner, SourceScope,
    SourceStart, SourceState, SourceStream, SourceStreamCounts, SourceTotals, StreamOnboarding,
    StreamPhase, TopicOwnership, UnhandledPolicy,
};
pub use sqlite::{ConnectionGuard, SCHEMA, SCHEMA_V2, SCHEMA_V3, SqliteAnnouncementStore};
pub use store::{AnnouncementStore, SubmissionCheck};
pub use types::{
    AnnounceOutcome, AnnounceRequest, Announcement, AnnouncementRef, AnnouncementStoreError,
    Announcer, Batch, BatchEvent, BatchEventTag, BatchId, BatchItem, BatchQuery, BatchState,
    BatchStateTag, ChildMemberLimits, LateOutcome, MAX_ITEM_WIRE_BYTES, MAX_REMOTE_BODY_WIRE_BYTES,
    Member, MemberBacklog, MemberIdentity, MemberRouteRecord, MobId, NativeReceipt, OperationId,
    PruneReport, PutSubscription, SOURCE_KEY_PREFIX, SealingPolicy, Selector, Settlement, SourceId,
    SourceKey, StartFrontier, Subscription, SubscriptionChange, SubscriptionChangeOutcome,
    SubscriptionDelivery, SubscriptionEnd, SubscriptionId, SubscriptionState, SuccessorBatch,
    Topic,
};

/// How long the dispatcher waits at startup for the first reconcile of the
/// declarations before it starts anyway.
const DECLARATIONS_RECONCILE_BEFORE_START: std::time::Duration = std::time::Duration::from_secs(30);

/// `runtime_options.mob_announcements`, as a host passes it at startup.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnnouncementOptions {
    enabled: bool,
    #[serde(default)]
    batch: Option<BatchOptions>,
    #[serde(default)]
    retention_ms: Option<u64>,
    #[serde(default)]
    member_limits: Option<MemberLimitOptions>,
    #[serde(default)]
    member_tools_in_child_mobs: bool,
    #[serde(default)]
    child_member_limits: Option<ChildMemberLimitOptions>,
    #[serde(default)]
    on_unhandled: Option<UnhandledPolicy>,
    #[serde(default)]
    renotify_after_ms: Option<u64>,
    #[serde(default)]
    health_audience: Option<Vec<sources::AudienceGrant>>,
    #[serde(default)]
    handled_unverified_threshold: Option<u32>,
    /// The sources and subscriptions this gateway declares, inline.
    #[serde(default)]
    declarations: Option<declarations::Declarations>,
    /// A packaged sidecar file holding them instead, read at init and at
    /// each `declarations/apply`.
    #[serde(default)]
    declarations_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildMemberLimitOptions {
    #[serde(default)]
    subscriptions: Option<u32>,
    #[serde(default)]
    per_mob: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberLimitOptions {
    #[serde(default)]
    announcements_per_minute: Option<u32>,
    #[serde(default)]
    announcement_bytes_per_minute: Option<u64>,
    #[serde(default)]
    subscriptions: Option<u32>,
    #[serde(default)]
    topics_per_subscription: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchOptions {
    #[serde(default)]
    max_count: Option<u32>,
    #[serde(default)]
    max_bytes: Option<u64>,
    #[serde(default)]
    max_delay_ms: Option<u64>,
    #[serde(default)]
    min_wake_interval_ms: Option<u64>,
}

/// Why a mob announcement configuration is refused before anything is
/// composed, on every surface that takes one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnouncementOptionsError {
    /// An option is malformed or out of range; the message names it.
    Invalid(String),
}

impl From<String> for AnnouncementOptionsError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl std::fmt::Display for AnnouncementOptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// Parse `runtime_options.mob_announcements`. `Ok(None)` when it is
/// disabled.
pub(crate) fn parse_runtime_options(
    value: &serde_json::Value,
) -> Result<Option<AnnouncementConfig>, AnnouncementOptionsError> {
    parse_options(value, "runtime_options.mob_announcements")
}

/// Parse a mob announcement options object (`{enabled, batch?,
/// retention_ms?, member_limits?, member_tools_in_child_mobs?,
/// child_member_limits?, on_unhandled?, renotify_after_ms?, health_audience?,
/// declarations?, declarations_path?}`), as every gateway takes it; `path`
/// is where the surface takes it, for the messages. `Ok(None)` when it is
/// disabled. The result has passed [`AnnouncementConfig::validate`]. A
/// sidecar of declarations is read here, once;
/// [`AnnouncementComposition::compose`] checks the declarations against
/// the gateway's mob.
pub fn parse_options(
    value: &serde_json::Value,
    path: &str,
) -> Result<Option<AnnouncementConfig>, AnnouncementOptionsError> {
    let options: AnnouncementOptions =
        serde_json::from_value(value.clone()).map_err(|error| format!("{path}: {error}"))?;
    if !options.enabled {
        return Ok(None);
    }
    let mut config = AnnouncementConfig {
        member_tools_in_child_mobs: options.member_tools_in_child_mobs,
        ..AnnouncementConfig::default()
    };
    if let Some(policy) = options.on_unhandled {
        config.on_unhandled = policy;
    }
    if let Some(batch) = options.batch {
        let policy = &mut config.policy;
        policy.max_count = batch.max_count.unwrap_or(policy.max_count);
        policy.max_bytes = batch.max_bytes.unwrap_or(policy.max_bytes);
        policy.max_delay_ms = batch.max_delay_ms.unwrap_or(policy.max_delay_ms);
        policy.min_wake_interval_ms = batch
            .min_wake_interval_ms
            .unwrap_or(policy.min_wake_interval_ms);
    }
    if let Some(retention_ms) = options.retention_ms {
        config.retention_ms = retention_ms;
    }
    if let Some(limits) = options.member_limits {
        let current = &mut config.member_limits;
        current.announcements_per_minute = limits
            .announcements_per_minute
            .unwrap_or(current.announcements_per_minute);
        current.announcement_bytes_per_minute = limits
            .announcement_bytes_per_minute
            .unwrap_or(current.announcement_bytes_per_minute);
        current.subscriptions = limits.subscriptions.unwrap_or(current.subscriptions);
        current.topics_per_subscription = limits
            .topics_per_subscription
            .unwrap_or(current.topics_per_subscription);
    }
    if let Some(limits) = options.child_member_limits {
        let current = &mut config.child_limits;
        current.subscriptions = limits.subscriptions.unwrap_or(current.subscriptions);
        current.per_mob = limits.per_mob.unwrap_or(current.per_mob);
    }
    if let Some(renotify_after_ms) = options.renotify_after_ms {
        config.renotify_after_ms = renotify_after_ms;
    }
    if let Some(health_audience) = options.health_audience {
        config.health_audience = health_audience;
    }
    if let Some(threshold) = options.handled_unverified_threshold {
        config.handled_unverified_threshold = threshold;
    }
    // One source of declarations per gateway: inline, or a sidecar read now.
    config.declarations = match (options.declarations, options.declarations_path) {
        (None, None) => None,
        (Some(declarations), None) => Some(Arc::new(DeclarationsSource::Inline(declarations))),
        (None, Some(sidecar)) => {
            let declarations = declarations::load_sidecar(&sidecar)
                .map_err(|message| format!("{path}.declarations_path: {message}"))?;
            Some(Arc::new(DeclarationsSource::Sidecar {
                path: sidecar,
                declarations,
            }))
        }
        (Some(_), Some(_)) => {
            return Err(AnnouncementOptionsError::Invalid(format!(
                "{path}: declarations and declarations_path are exclusive; a gateway has one \
                 source of declarations"
            )));
        }
    };
    config
        .validate()
        .map_err(|AnnouncementOptionsError::Invalid(message)| {
            AnnouncementOptionsError::Invalid(format!("{path}.{message}"))
        })?;
    Ok(Some(config))
}

/// Where a runtime keeps its mob announcement owner once one attaches. A
/// surface built before the owner attaches (the console) reads it at each
/// request.
pub type AnnouncementSlot = Arc<std::sync::OnceLock<AnnouncementService>>;

/// Mob announcements as a host enables them, composed the same way on every
/// surface: `rpc_gateway` from `runtime_options.mob_announcements`,
/// `meta_gateway` from its init's `mob_announcements`, and a runtime the
/// builder composes from
/// [`UnifiedRuntimeBuilder::mob_announcements`](crate::UnifiedRuntimeBuilder::mob_announcements).
///
/// Two steps, in this order: [`Self::register_member_tools`] puts the member
/// tool bundle on the mob spec before the runtime is built, and
/// [`Self::compose`] attaches the owner once the runtime's identity runtime
/// is installed, before the keyed inject plane of its control listener is
/// sealed.
pub struct AnnouncementComposition {
    config: AnnouncementConfig,
    member_tools: tools::ToolSlot,
}

impl AnnouncementComposition {
    /// Refuses a configuration [`AnnouncementConfig::validate`] refuses.
    pub fn new(config: AnnouncementConfig) -> Result<Self, AnnouncementOptionsError> {
        config.validate()?;
        Ok(Self {
            config,
            member_tools: Arc::new(std::sync::OnceLock::new()),
        })
    }

    /// The window a peer applies to this gateway's keyed inject requests: a
    /// batch for a member on a peer expires only once it has passed. Each
    /// surface sets the window its own control listener serves with.
    #[must_use]
    pub fn with_peer_request_window(mut self, window_ms: u64) -> Self {
        self.config.peer_request_window_ms = window_ms;
        self
    }

    /// Put the member tool bundle ([`tools::TOOL_BUNDLE`]) on `spec`, before
    /// the runtime is built and so before any child mob can be created. The
    /// host's own profiles that name it in `tools.rust_bundles` get the
    /// member tools. Under the ceiling
    /// ([`AnnouncementConfig::member_tools_in_child_mobs`]) it is offered to
    /// child mobs, and a child mob profile mounts it only by naming it in
    /// its `tools.bundles`; otherwise it is the host's alone.
    pub fn register_member_tools(
        &self,
        spec: crate::mob_handle_runtime::MobBootstrapSpec,
    ) -> crate::mob_handle_runtime::MobBootstrapSpec {
        let (name, dispatcher, availability) = self.member_tool_bundle();
        spec.register_tool_bundle_with_availability(name, dispatcher, availability)
    }

    /// The member tool bundle's name, dispatcher and availability to child
    /// mobs, for a composer that collects its bundles before it resolves a
    /// spec: offered under the ceiling, never supplied to every child
    /// profile, and the host's alone otherwise.
    pub(crate) fn member_tool_bundle(
        &self,
    ) -> (
        &'static str,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
        meerkat_mob_mcp::ChildToolBundleAvailability,
    ) {
        let availability = if self.config.member_tools_in_child_mobs {
            meerkat_mob_mcp::ChildToolBundleAvailability::ChildOffered
        } else {
            meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly
        };
        (
            tools::TOOL_BUNDLE,
            Arc::new(tools::MobAnnouncementTools::new(Arc::clone(
                &self.member_tools,
            ))),
            availability,
        )
    }

    /// Attach the owner to `runtime`: open its database under `layout`,
    /// bind the member tools, start the dispatcher and follow the mob set,
    /// so destroyed mobs leave the subscriptions that name them. The runtime
    /// must be identity-first; its shutdown stops the dispatcher.
    ///
    /// A runtime bootstrapped for a candidate launch, one that does not speak
    /// for its mob composition, gets the owner quiesced: no dispatcher, no
    /// declarations reconcile and no mob set to follow, and every change is
    /// refused with [`AnnouncementError::CandidateLaunch`]. Reads still
    /// answer.
    pub fn compose(
        self,
        runtime: &crate::unified_runtime::UnifiedRuntime,
        layout: &crate::storage_layout::MetaStorageLayout,
    ) -> Result<AnnouncementService, String> {
        self.compose_on(runtime, StoreLocation::StateDirectory(layout))
    }

    /// [`Self::compose`] over a host store realm, which keeps the store's
    /// tables (see the `host_store` module): the process works on an
    /// in-memory copy, and every changing call is one batch the realm holds
    /// before the call answers. Opening the store writes nothing, so it opens
    /// before anything else is composed.
    pub(crate) async fn compose_over_host_store(
        self,
        runtime: &crate::unified_runtime::UnifiedRuntime,
        realm: &Arc<meerkat_runtime::store::spi::SpiRealm>,
    ) -> Result<AnnouncementService, String> {
        let store = storage::open_host_store(realm)
            .await
            .map_err(|error| format!("mob announcement store: {error}"))?;
        self.compose_on(runtime, StoreLocation::HostStore(store))
    }

    fn compose_on(
        self,
        runtime: &crate::unified_runtime::UnifiedRuntime,
        location: StoreLocation<'_>,
    ) -> Result<AnnouncementService, String> {
        let Self {
            config,
            member_tools,
        } = self;
        let local = Arc::new(local_delivery(runtime)?);
        let identities = runtime
            .identity_runtime_ref()
            .cloned()
            .ok_or("mob announcements require an identity-first gateway")?;
        let own_mob = MobId::new(runtime.mob_handle().mob_id().to_string())
            .map_err(|error| format!("this gateway's mob id: {error}"))?;
        // Declarations a static check refuses fail the gateway closed, as a
        // refused announcement configuration does, before the store opens (a
        // host store realm's store opened already, and wrote nothing).
        if let Some(source) = config.declarations.as_deref() {
            declarations::validate(source.declarations(), &own_mob)
                .map_err(|error| format!("mob announcement declarations: {error}"))?;
        }
        let store = match location {
            StoreLocation::StateDirectory(layout) => {
                let path = layout
                    .mob_announcements_db()
                    .map_err(|error| format!("mob announcement database: {error}"))?;
                storage::open_disk_store(path)
                    .map_err(|error| format!("mob announcement database: {error}"))?
            }
            StoreLocation::HostStore(store) => store,
        };
        let window_ms = config.peer_request_window_ms;
        let remote = runtime
            .peer_delivery_access()
            .map(|(contacts, keys)| Arc::new(RemoteMemberDelivery::new(contacts, keys, window_ms)));
        // Callers in mobs members created are attributed through this
        // gateway's runtime only when they can be offered the tools.
        let children = config
            .member_tools_in_child_mobs
            .then(|| Arc::clone(&local));
        let native = AnnouncementNativeDelivery::new(local, remote);
        let member_limits = config.member_limits;
        let service = AnnouncementService::new(Arc::new(store), Arc::new(native), config)
            .map_err(|error| error.to_string())?;
        // A candidate launch (one that does not speak for the runtime's mob
        // composition) gets an owner quiesced before any surface can reach
        // it: it delivers nothing and refuses every change, and reads still
        // answer.
        let candidate = !runtime.speaks_for_composition();
        if candidate {
            service.quiesce_for_candidate();
        }
        // Escalation events reach the host through the gateway's error hook.
        service.set_error_sink(runtime.error_sink());
        if !runtime.attach_mob_announcements(service.clone()) {
            return Err("mob announcements are already attached to this runtime".to_string());
        }
        // A non-owning handle: the member tools never keep the mob open.
        let callers = tools::RosterCallerResolver::new(
            own_mob,
            runtime.mob_handle().observer(),
            identities,
            children,
        );
        let bound = member_tools
            .set(tools::ToolBackend {
                service: service.clone(),
                callers: Arc::new(callers),
                limits: member_limits,
            })
            .is_ok();
        if !bound {
            return Err("the member tools are already bound".to_string());
        }
        if candidate {
            // No dispatcher, no declarations reconcile and no mob watch: each
            // would deliver or write what the authoritative launch owns.
            tracing::info!(
                "mob announcements quiesced for this candidate launch: no batch is sealed or \
                 delivered, no declaration is reconciled, and every change is refused; queued \
                 work stays for the authoritative launch, and reads still answer"
            );
            return Ok(service);
        }
        // The dispatcher starts once the declarations are reconciled (the
        // options' ones first, when newer than the applied ones), so its
        // first round runs on what they made and dropped. A reconcile that
        // fails or outlasts its bound is logged and the dispatcher starts
        // anyway; later triggers reconcile again, and a runtime refusal
        // waits as pending.
        let starting = service.clone();
        drop(tokio::spawn(async move {
            drop(
                starting
                    .start_after_reconcile(DECLARATIONS_RECONCILE_BEFORE_START)
                    .await,
            );
        }));
        drop(service.watch_mobs());
        Ok(service)
    }
}

/// Where a composition's announcement store lives.
enum StoreLocation<'a> {
    /// The database in a state directory's layout, opened by the composition.
    StateDirectory(&'a crate::storage_layout::MetaStorageLayout),
    /// The store of a host store realm, opened already.
    HostStore(SqliteAnnouncementStore),
}

/// The seam to this gateway's own runtime: its identity runtime, its agent
/// mob tool state and its runtime machine.
fn local_delivery(
    runtime: &crate::unified_runtime::UnifiedRuntime,
) -> Result<MobNativeDelivery, String> {
    let identities = runtime
        .identity_runtime_ref()
        .cloned()
        .ok_or("mob announcements require an identity-first gateway")?;
    let mob_runtime = runtime.mob_runtime();
    let mob_tools = mob_runtime
        .agent_mob_mcp_state()
        .ok_or("mob announcements require the gateway's agent mob tool state")?;
    let sessions = mob_runtime
        .session_service()
        .cloned()
        .ok_or("mob announcements require the gateway's session service")?;
    let machine = crate::mob_handle_runtime::observed_runtime_adapter(sessions.as_ref())
        .ok_or("mob announcements require the gateway's runtime machine")?;
    let mob_id = MobId::new(runtime.mob_handle().mob_id().to_string())
        .map_err(|error| format!("this gateway's mob id: {error}"))?;
    Ok(MobNativeDelivery::new(
        mob_id, identities, mob_tools, machine, sessions,
    ))
}

/// How a gateway's configuration has its control channel serve the keyed
/// inject plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyedInjectPlane {
    /// Serve peers' keyed injects, refusing a request issued further than
    /// `window_ms` from this gateway's clock.
    Serve { window_ms: u64 },
    /// Serve none: both verbs answer `not_served` with the cause
    /// `runtime_disabled`.
    Disabled,
}

impl Default for KeyedInjectPlane {
    fn default() -> Self {
        Self::Serve {
            window_ms: crate::runtime::keyed_inject::DEFAULT_KEYED_INJECT_WINDOW_MS,
        }
    }
}

/// Seal the keyed inject plane of a gateway whose control listener is bound,
/// once its runtime is composed: every surface (`rpc_gateway`,
/// `meta_gateway` and the runtime builder) composes it here and nowhere
/// else.
///
/// An identity-first gateway that serves keyed injects submits what a peer
/// delivers to one of its members to its own continuation owner, whether or
/// not it enables mob announcements itself. Any other gateway answers
/// `not_served` with its typed cause. Until this runs, both verbs answer
/// `keyed_inject_unavailable`: the gateway is starting.
pub fn compose_keyed_inject_plane(
    runtime: &crate::unified_runtime::UnifiedRuntime,
    plane: KeyedInjectPlane,
) -> Result<(), String> {
    use crate::runtime::keyed_inject::{KeyedInjectServing, NotServedCause};
    let serving = match plane {
        KeyedInjectPlane::Disabled => {
            KeyedInjectServing::NotServed(NotServedCause::RuntimeDisabled)
        }
        KeyedInjectPlane::Serve { .. } if runtime.identity_runtime_ref().is_none() => {
            KeyedInjectServing::NotServed(NotServedCause::NotIdentityFirst)
        }
        KeyedInjectPlane::Serve { window_ms } => {
            let local = Arc::new(local_delivery(runtime)?);
            KeyedInjectServing::Serving(Arc::new(KeyedInjectServer::new(local, window_ms)))
        }
    };
    if !runtime.seal_keyed_inject(serving) {
        return Err("the keyed inject plane of this runtime is already sealed".to_string());
    }
    Ok(())
}
