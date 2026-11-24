//! Reusable assembly for the FULL agent-memory stack (§6 architecture): a
//! judgment-capable provider with the §10.1 taint firewall, plus the
//! judgment plane (Distiller, Steward) and the member-event observer feeding
//! them.
//!
//! Until this module existed the stack was hand-assembled inside
//! `rpc_gateway.rs`, which made the judgment plane unreachable for Rust
//! embedders driving `UnifiedRuntimeBuilder` directly (the production deployment
//! shape: no gateways, no SDKs — builder or nothing). The builder now exposes
//! it through [`crate::UnifiedRuntimeBuilder::persistent_agent_memory_stack`].
//!
//! M4 de-weld: assembly is capability-driven, not store-kind-driven. The
//! provider parameter is the plain [`AgentMemoryProvider`]; each piece of the
//! plane probes for the capability it needs ([`TaintableStore`] for the
//! firewall, [`StewardStore`] for the Steward, `TombstoneSource` for the
//! Distiller, [`MemoryPanelStore`] for the console panel handle on the
//! returned stack) and fails with a named error when a requested engine's
//! capability is missing. An externally supplied recall-only provider is
//! recall-only because it advertises none of these capabilities.
//!
//! Scope notes, deliberate for v1:
//! - The **Hygienist** engine stays internal and parked. Public SDK/gateway
//!   activation is refused until the release has bounded projection and real
//!   provider proof; the builder does not acquire an activation seam here.
//! - The gateway keeps its existing hand wiring (identical semantics, more
//!   seams: fail-init surfacing, gating/conflict bridges, schedule-host dream
//!   suppression). Converging it onto this module is tracked follow-up work.
//!
//! [`TaintableStore`]: crate::memory::capabilities::TaintableStore
//! [`StewardStore`]: crate::memory::capabilities::StewardStore
//! [`MemoryPanelStore`]: crate::memory::capabilities::MemoryPanelStore

use std::path::Path;
use std::sync::Arc;

use crate::MemberAgentEventSink;
use crate::identity_first::agent_memory::{AgentMemoryConfig, AgentMemoryProvider};
use crate::memory::capabilities::{MemoryPanelStore, StewardStore};
use crate::memory::distiller::{
    DistillerConfig, DistillerEngine, DistillerError, DistillerProfile, DistillerTriggers,
    FactoryDistillerHandle, HnswDiscardSource, SessionStoreTranscriptSource, TranscriptSource,
};
use crate::memory::events::MemoryEventSink;
use crate::memory::sqlite_store::SqliteAgentMemoryStore;
use crate::memory::steward::{
    FactoryStewardHandle, MemoryConflictBridge, MemoryGatingBridge, MobPurposeSource,
    PrivateMemberSource, SessionStoreEvidenceResolver, StewardConfig, StewardEngine,
    StewardProfile, StewardTriggers,
};
use crate::memory::taint::{SessionTaintTracker, TaintLlmWriteGate};

/// Engine enables + models for the judgment plane. `Default` = store +
/// firewall only (no background LLM work), matching the architecture's
/// "each stage ships alone" posture.
#[derive(Debug, Clone, Default)]
pub struct MemoryEnginesConfig {
    pub distiller: DistillerConfig,
    pub steward: StewardConfig,
}

/// Resolve the exact Distiller profile used by stack assembly.
///
/// This is public so gateway contract tests can prove that host JSON reaches
/// the same profile construction path as a live engine. Runtime callers should
/// normally use [`attach_memory_engines`] instead.
#[doc(hidden)]
pub fn effective_distiller_profile(config: &DistillerConfig) -> Result<DistillerProfile, String> {
    let mut profile = DistillerProfile::embedded_default();
    if let Some(model) = config.model.as_deref() {
        profile = profile
            .with_model_override(model)
            .map_err(|e| format!("agent memory distiller: {e}"))?;
    }
    if let Some(max_output_tokens) = config.max_output_tokens {
        profile = profile
            .with_max_output_tokens(max_output_tokens)
            .map_err(|e| format!("agent memory distiller: {e}"))?;
    }
    Ok(profile)
}

/// Resolve the exact Steward profile used by stack assembly.
///
/// Kept visible for the same gateway-to-wiring contract proof as
/// [`effective_distiller_profile`].
#[doc(hidden)]
pub fn effective_steward_profile(config: &StewardConfig) -> Result<StewardProfile, String> {
    let mut profile = StewardProfile::embedded_default();
    if let Some(model) = config.model.as_deref() {
        profile = profile
            .with_model_override(model)
            .map_err(|e| format!("agent memory steward: {e}"))?;
    }
    if let Some(max_output_tokens) = config.max_output_tokens {
        profile = profile
            .with_max_output_tokens(max_output_tokens)
            .map_err(|e| format!("agent memory steward: {e}"))?;
    }
    Ok(profile)
}

/// Host-supplied seams for the stack: where the engines read transcripts,
/// where events project, and the steward's late-binding bridges. Everything
/// optional degrades gracefully (engines that need a missing seam fail the
/// build with a named error; bridges just don't bind).
#[derive(Default)]
pub struct MemoryStackSeams {
    pub persistent_state: Option<std::path::PathBuf>,
    pub transcript_store: Option<Arc<dyn meerkat::SessionStore>>,
    pub event_sink: Option<Arc<dyn MemoryEventSink>>,
    pub mob_purpose: Option<Arc<dyn MobPurposeSource>>,
    /// The members private to a person, whose memory the Steward dreams
    /// over none of.
    pub private_members: Option<Arc<dyn crate::memory::steward::PrivateMemberSource>>,
    pub steward_gating: Option<Arc<dyn MemoryGatingBridge>>,
    pub steward_conflicts: Option<Arc<dyn MemoryConflictBridge>>,
}

/// The assembled stack. The caller finishes the wiring that needs the live
/// runtime: register `panel` as the console panel store (when advertised),
/// pass `sinks` to [`crate::spawn_member_event_observer`], hand
/// `taint`/engines to the injector, and start the steward dream (schedule
/// host or `StewardEngine::spawn_dream_loop`).
pub struct AgentMemoryStack {
    pub provider: Arc<dyn AgentMemoryProvider>,
    /// The provider's steward read/write surface, when advertised. Present
    /// independent of whether the Steward engine is enabled — gateway
    /// extras (e.g. the Hygienist's span-reference source) read through it.
    pub steward_store: Option<Arc<dyn StewardStore>>,
    /// The provider's console Memory panel read API, when advertised.
    pub panel: Option<Arc<dyn MemoryPanelStore>>,
    pub taint: SessionTaintTracker,
    pub distiller: Option<Arc<DistillerEngine>>,
    pub steward: Option<Arc<StewardEngine>>,
    /// Observe-stream consumers (taint + engine triggers), in wiring order.
    pub sinks: Vec<Arc<dyn MemberAgentEventSink>>,
}

/// Open the bundled SQLite store with the §10.1 firewall and assemble the
/// enabled engines. `persistent_state` is the runtime's state dir (engine
/// LLM factories and the compaction-discard source live under it);
/// `transcript_store` is a read handle on the session store the mob bridge
/// persists to (required when either engine is enabled).
pub fn build_sqlite_memory_stack(
    memory_dir: &Path,
    config: &AgentMemoryConfig,
    engines: &MemoryEnginesConfig,
    seams: MemoryStackSeams,
) -> Result<AgentMemoryStack, String> {
    let store = SqliteAgentMemoryStore::open(memory_dir)
        .map_err(|e| format!("failed to open agent memory store: {e}"))?;
    attach_memory_engines(Arc::new(store), config, engines, seams)
}

/// Stage-2 assembly over an already-open provider: wire the §10.1 firewall
/// (tracker + write gate + event sinks) and the enabled engines. Used by the
/// builder path, where the provider doubles as the one handed to the
/// customizer before the runtime exists.
///
/// Capability requirements: the firewall needs [`TaintableStore`] (always —
/// the Recorder must not ship without it); the Distiller additionally needs
/// tombstone reads; the Steward additionally needs [`StewardStore`]. Each
/// missing capability is a named error, never a silent downgrade.
///
/// The engines resolve their provider credentials in the process default
/// store; [`attach_memory_engines_with_provider_auth`] names the runtime's.
///
/// [`TaintableStore`]: crate::memory::capabilities::TaintableStore
pub fn attach_memory_engines(
    provider: Arc<dyn AgentMemoryProvider>,
    config: &AgentMemoryConfig,
    engines: &MemoryEnginesConfig,
    seams: MemoryStackSeams,
) -> Result<AgentMemoryStack, String> {
    attach_memory_engines_with_provider_auth(provider, config, engines, seams, None)
}

/// [`attach_memory_engines`] over the runtime's provider credential store:
/// with `provider_auth_persistence`, the Distiller and the Steward resolve
/// and persist their provider credentials there, with the refresh authority
/// the runtime's agents share, instead of in the process default store.
/// `None` keeps the process default.
pub fn attach_memory_engines_with_provider_auth(
    provider: Arc<dyn AgentMemoryProvider>,
    config: &AgentMemoryConfig,
    engines: &MemoryEnginesConfig,
    seams: MemoryStackSeams,
    provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
) -> Result<AgentMemoryStack, String> {
    assemble_memory_stack(
        provider,
        config,
        engines,
        seams,
        provider_auth_persistence,
        MemoryStackOver::default(),
    )
}

/// What a launch assembles a stack over where it does not let the assembly
/// make its own ([`assemble_memory_stack`]).
#[derive(Default)]
pub(crate) struct MemoryStackOver {
    /// The tracker the store's write gate answers from: one that outlives
    /// this assembly, as the tracker of a mob that is unloaded and loaded
    /// again does. `None`: a tracker of this stack's own.
    pub(crate) taint: Option<SessionTaintTracker>,
    /// The ONE transcript source the Distiller, the Steward and the
    /// evidence resolver read through. `None`: a source over the seam's
    /// session store that reads every session of it.
    pub(crate) transcripts: Option<Arc<dyn TranscriptSource>>,
    /// Test builds only: the model clients the engines are built with in
    /// the place of the factory's. `None`: the factory's.
    #[cfg(test)]
    pub(crate) clients: Option<EngineClientsInTests>,
}

/// [`attach_memory_engines_with_provider_auth`] over what the launch
/// brings itself ([`MemoryStackOver`]). The launches assemble their own
/// stacks here: with the transcript source that reads the sessions of the
/// launch's memory alone ([`launch_transcript_source`]), and, for a mob
/// hosted beside the launch mob, over that mob's lasting tracker.
pub(crate) fn assemble_memory_stack(
    provider: Arc<dyn AgentMemoryProvider>,
    config: &AgentMemoryConfig,
    engines: &MemoryEnginesConfig,
    seams: MemoryStackSeams,
    provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
    over: MemoryStackOver,
) -> Result<AgentMemoryStack, String> {
    let MemoryStackSeams {
        persistent_state,
        transcript_store,
        event_sink,
        mob_purpose,
        private_members,
        steward_gating,
        steward_conflicts,
    } = seams;
    let persistent_state = persistent_state.as_deref();
    let event_sink =
        event_sink.ok_or_else(|| "agent memory stack requires an event sink".to_string())?;
    let taintable = provider.as_taintable().ok_or_else(|| {
        "agent memory stack requires a provider with firewall controls (TaintableStore)".to_string()
    })?;
    // §10.1 taint firewall: the Recorder must not ship without it. This
    // deliberately REPLACES any trackerless posture gate installed earlier.
    let taint = over
        .taint
        .unwrap_or_else(|| SessionTaintTracker::new(config.content_trust.clone()));
    taintable.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
        Some(taint.clone()),
        config.llm_writes,
    )));
    taintable.set_event_sink(event_sink.clone());
    taint.set_event_sink(event_sink.clone());

    let mut sinks: Vec<Arc<dyn MemberAgentEventSink>> = vec![Arc::new(taint.clone())];
    let realm = config.realm.clone();

    let state_for_engines = |what: &str| -> Result<&Path, String> {
        persistent_state.ok_or_else(|| format!("agent memory {what} requires persistent_state"))
    };
    #[cfg(test)]
    let clients_in_tests = over.clients;
    // The one transcript source of this stack: the one it is assembled
    // over, else one over the seam's session store.
    let one_source: Option<Arc<dyn TranscriptSource>> = over.transcripts.or_else(|| {
        transcript_store.map(|store| {
            Arc::new(SessionStoreTranscriptSource::new(store)) as Arc<dyn TranscriptSource>
        })
    });
    let transcripts_for_engines = |what: &str| -> Result<Arc<dyn TranscriptSource>, String> {
        one_source
            .clone()
            .ok_or_else(|| format!("agent memory {what} requires a session transcript store"))
    };

    let distiller = if engines.distiller.enabled {
        let state = state_for_engines("distiller")?;
        let transcripts = transcripts_for_engines("distiller")?;
        let tombstones = provider.as_tombstone_source().ok_or_else(|| {
            "agent memory distiller requires a provider with tombstone reads (TombstoneSource)"
                .to_string()
        })?;
        let profile = effective_distiller_profile(&engines.distiller)?;
        let mut handle =
            FactoryDistillerHandle::new(state, meerkat::Config::default(), &realm, &profile);
        if let Some(persistence) = provider_auth_persistence.clone() {
            handle = handle.with_provider_auth_persistence(persistence);
        }
        let handle: Arc<dyn crate::memory::distiller::DistillerClientHandle> = Arc::new(handle);
        #[cfg(test)]
        let handle = clients_in_tests
            .as_ref()
            .map_or(handle, |clients| Arc::clone(&clients.distiller));
        let engine = Arc::new(DistillerEngine::new(
            profile,
            engines.distiller.clone(),
            handle,
            provider.clone(),
            tombstones,
            transcripts,
            // Meerkat's session semantic memory lives at
            // <persistent_state>/memory; absent dir ⇒ nothing preserved.
            Some(Arc::new(HnswDiscardSource::new(state.join("memory")))),
            Some(taint.clone()),
            realm.clone(),
        ));
        engine.set_event_sink(event_sink.clone());
        sinks.push(Arc::new(DistillerTriggers::new(engine.clone())));
        Some(engine)
    } else {
        None
    };

    let steward_store = provider.as_steward_store();
    let steward = if engines.steward.enabled {
        let store = steward_store.clone().ok_or_else(|| {
            "agent memory steward requires a provider with the steward surface (StewardStore)"
                .to_string()
        })?;
        let state = state_for_engines("steward")?;
        let transcripts_source = transcripts_for_engines("steward")?;
        let profile = effective_steward_profile(&engines.steward)?;
        // §10.2 P3 validator extension: agent_verified retiers must cite
        // evidence that resolves against the session store.
        taintable.set_evidence_resolver(Arc::new(SessionStoreEvidenceResolver::new(
            transcripts_source.clone(),
            tokio::runtime::Handle::current(),
        )));
        let mut handle = FactoryStewardHandle::new(
            state.to_path_buf(),
            meerkat::Config::default(),
            realm.clone(),
            &profile,
        );
        if let Some(persistence) = provider_auth_persistence {
            handle = handle.with_provider_auth_persistence(persistence);
        }
        let handle: Arc<dyn crate::memory::steward::StewardClientHandle> = Arc::new(handle);
        #[cfg(test)]
        let handle = clients_in_tests
            .as_ref()
            .map_or(handle, |clients| Arc::clone(&clients.steward));
        let mut engine = StewardEngine::new(
            profile,
            engines.steward.clone(),
            handle,
            store,
            transcripts_source,
            realm,
        )
        .with_events(event_sink.clone())
        .with_operator_routing(
            config.operator_scope
                == crate::identity_first::agent_memory::AgentMemoryOperatorScope::Provisional,
        );
        if let Some(purpose) = mob_purpose {
            engine = engine.with_mob_context(purpose);
        }
        if let Some(private_members) = private_members {
            engine = engine.with_private_members(private_members);
        }
        if let Some(gating) = steward_gating {
            engine = engine.with_gating(gating);
        }
        if let Some(conflicts) = steward_conflicts {
            engine = engine.with_conflicts(conflicts);
        }
        let engine = Arc::new(engine);
        sinks.push(Arc::new(StewardTriggers::new(engine.clone())));
        Some(engine)
    } else {
        None
    };

    Ok(AgentMemoryStack {
        steward_store,
        panel: provider.as_memory_panel_store(),
        provider,
        taint,
        distiller,
        steward,
        sinks,
    })
}

/// The transcript source of a launch's own memory engines: the sessions of
/// `store` that are sessions of the launch's memory, and no other.
///
/// A launch's session store holds the sessions of every mob the launch
/// builds, and the engines are given session ids by a model. A session is
/// the launch's when a journal binds it ([`MemoryTranscripts`]): the
/// journal of the mob its stored member binding names, when that mob's
/// members are members of the launch's memory
/// ([`crate::private_owners::MemoryHome::Launch`]); and, for a session
/// whose stored binding names no mob, the journal of `launch_mob`, the
/// launch's own mob. No time, and no label, name or text of an agent or a
/// model is asked.
pub(crate) fn launch_transcript_source(
    store: Arc<dyn meerkat::SessionStore>,
    owners: crate::private_owners::PrivateOwners,
    launch_mob: meerkat_mob::MobHandle,
) -> Arc<dyn TranscriptSource> {
    launch_transcript_source_over(
        store,
        launch_mob.mob_id().clone(),
        Arc::new(launch_mob),
        Arc::new(owners),
    )
}

/// [`launch_transcript_source`] over what tells which sessions the launch
/// mob's journal bound, and over the launch's record of its other mobs.
pub(crate) fn launch_transcript_source_over(
    store: Arc<dyn meerkat::SessionStore>,
    launch_mob: meerkat_mob::MobId,
    journal: Arc<dyn BoundSessions>,
    mobs: Arc<dyn MobsOfTheLaunchMemory>,
) -> Arc<dyn TranscriptSource> {
    Arc::new(MemoryTranscripts {
        of_the_store: SessionStoreTranscriptSource::new(store),
        home: TranscriptsOf::TheLaunch {
            launch_mob,
            journal,
            mobs,
        },
    })
}

/// Whether the record `owners` names the mob `launch_mob` the launch's own:
/// asked once by a launch before it assembles its memory engines over
/// [`launch_transcript_source`]. A launch whose record does not would have
/// engines that read none of its own members' sessions, without a word, so
/// it fails here with a named error instead.
pub(crate) fn launch_mob_is_bound(
    owners: &crate::private_owners::PrivateOwners,
    launch_mob: &meerkat_mob::MobId,
) -> Result<(), String> {
    match owners.memory_home_of(launch_mob.as_str()) {
        crate::private_owners::MemoryHome::Launch => Ok(()),
        home => Err(format!(
            "agent memory engines require the launch's record of its mobs to name the launch \
             mob '{launch_mob}' its own; it answers {home:?}"
        )),
    }
}

/// A launch's own mob keeps no journal of its members' sessions: the
/// storage a host composed for it has no identity and member store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchMobKeepsNoJournal {
    pub(crate) mob: meerkat_mob::MobId,
}

impl std::fmt::Display for LaunchMobKeepsNoJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the storage of the mob '{}' keeps no journal of its members' sessions (it has no \
             identity and member store), so which sessions are the launch's own cannot be read",
            self.mob
        )
    }
}

/// Whether the storage of the launch's own mob keeps the journal of its
/// members' sessions: asked once by a launch, of its mob's handle, before
/// it assembles its memory engines over [`launch_transcript_source`]. The
/// engines of a launch whose mob storage has no such journal could not
/// tell which session is one of their own and would read none, without a
/// word, so the launch is refused here with a named reason instead, which
/// each launch words for whoever composed it.
///
/// Only that answer of the storage refuses. A read that fails for another
/// reason is no fact of the composition: the source answers it for each
/// session it is asked.
pub(crate) async fn launch_mob_keeps_a_journal(
    launch_mob: &meerkat_mob::MobHandle,
) -> Result<(), LaunchMobKeepsNoJournal> {
    let Err(meerkat_mob::MobError::StorageError(error)) = launch_mob.member_binding_history().await
    else {
        return Ok(());
    };
    if matches!(
        error.downcast_ref::<meerkat_mob::MobStoreError>(),
        Some(meerkat_mob::MobStoreError::IdentityMemberAtomicPersistenceUnavailable)
    ) {
        return Err(LaunchMobKeepsNoJournal {
            mob: launch_mob.mob_id().clone(),
        });
    }
    Ok(())
}

/// The transcript source of the memory engines of the mob of `mob`, a mob a
/// launch hosts beside its launch mob: the sessions of `store` that are
/// that mob's own, and no other ([`MemoryTranscripts`], with that mob as
/// its home: every session is asked of that mob's journal).
pub(crate) fn hosted_mob_transcript_source(
    store: Arc<dyn meerkat::SessionStore>,
    mob: meerkat_mob::MobHandle,
) -> Arc<dyn TranscriptSource> {
    hosted_mob_transcript_source_over(store, Arc::new(mob))
}

/// [`hosted_mob_transcript_source`] over what tells which sessions the
/// mob's journal bound.
pub(crate) fn hosted_mob_transcript_source_over(
    store: Arc<dyn meerkat::SessionStore>,
    journal: Arc<dyn BoundSessions>,
) -> Arc<dyn TranscriptSource> {
    Arc::new(MemoryTranscripts {
        of_the_store: SessionStoreTranscriptSource::new(store),
        home: TranscriptsOf::AHostedMob(journal),
    })
}

/// The member a mob's journal bound a session to, as far as a memory's
/// engines ask of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundMember {
    /// Whether the member is private to a person, by the labels its binding
    /// was spawned with ([`crate::access::private_owner`]).
    pub(crate) private: bool,
}

/// Which sessions the journal of one mob bound: the mob's own handle, in a
/// launch.
#[async_trait::async_trait]
pub(crate) trait BoundSessions: Send + Sync {
    /// The member the mob's journal bound to `session` last, `None` when it
    /// never bound it. An error when the journal cannot be read: that is
    /// not "never bound".
    async fn bound(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<Option<BoundMember>, String>;
}

#[async_trait::async_trait]
impl BoundSessions for meerkat_mob::MobHandle {
    async fn bound(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<Option<BoundMember>, String> {
        self.member_binding_for_session(session)
            .await
            .map(|binding| {
                binding.map(|binding| BoundMember {
                    private: crate::access::private_owner(binding.labels()).is_some(),
                })
            })
            .map_err(|error| error.to_string())
    }
}

/// The launch's record of the mobs whose members are members of its memory
/// beside its launch mob: the mobs a launch mob's member created, at any
/// depth.
#[async_trait::async_trait]
pub(crate) trait MobsOfTheLaunchMemory: Send + Sync {
    /// Whose memory the members of the mob `mob_id` are members of.
    fn home_of(&self, mob_id: &str) -> crate::private_owners::MemoryHome;

    /// The journal of the mob that stands under `mob_id` now, when that mob
    /// is loaded. `None` for a mob that is not loaded: it is never loaded,
    /// and no store of it is opened, for this question.
    async fn loaded_journal_of(
        &self,
        mob_id: &str,
    ) -> Result<Option<Arc<dyn BoundSessions>>, String>;
}

#[async_trait::async_trait]
impl MobsOfTheLaunchMemory for crate::private_owners::PrivateOwners {
    fn home_of(&self, mob_id: &str) -> crate::private_owners::MemoryHome {
        self.memory_home_of(mob_id)
    }

    async fn loaded_journal_of(
        &self,
        mob_id: &str,
    ) -> Result<Option<Arc<dyn BoundSessions>>, String> {
        Ok(self
            .loaded_mob(mob_id)
            .await?
            .map(|handle| Arc::new(handle) as Arc<dyn BoundSessions>))
    }
}

/// Whose memory a transcript source reads for: which journal says that a
/// session of the store is that memory's own.
enum TranscriptsOf {
    /// The launch's memory. A session whose stored member binding names a
    /// mob is asked of that mob's journal, when the mob's members are
    /// members of the launch's memory; a session whose stored binding
    /// names no mob, and an id the store holds no session under, of the
    /// launch mob's journal. The stored binding only chooses the journal:
    /// that a session is a member's at all, and which member's, is the
    /// journal's answer.
    TheLaunch {
        launch_mob: meerkat_mob::MobId,
        journal: Arc<dyn BoundSessions>,
        mobs: Arc<dyn MobsOfTheLaunchMemory>,
    },
    /// The memory of a mob the launch hosts beside its launch mob: every
    /// session is asked of that mob's journal.
    AHostedMob(Arc<dyn BoundSessions>),
}

impl TranscriptsOf {
    /// Whose journals did not bind a session this source does not hold, in
    /// the words of its answer.
    fn journals(&self) -> &'static str {
        match self {
            Self::TheLaunch { .. } => "a journal of the launch's memory",
            Self::AHostedMob(_) => "this mob's journal",
        }
    }
}

/// The transcript source of one memory's engines: the launch's session
/// store, read for the sessions a journal of that memory bound.
///
/// The launch's session store holds the sessions of every mob the launch
/// builds, and the engines are given session ids by a model. Which session
/// is a memory's own is what a mob's own journal bound: a session is read
/// when the journal of the mob that stands under the id now bound a member
/// to it ([`meerkat_mob::MobHandle::member_binding_for_session`]: a seat,
/// a reset or a recovered binding, of a member that lives or retired). A
/// destroy clears a mob's journal, so a mob created under the id of a
/// destroyed one binds none of that mob's sessions. No time is asked.
///
/// A session no such journal bound reads as one the store does not hold,
/// and whether it is private to a person is unknown (an error), never
/// "private to no one": a run that takes its evidence beside this source
/// (the discards of a compaction) asks that question first, and distils
/// nothing when it has no answer. An id the store holds no session under
/// is answered the same, word for word. A journal that cannot be read
/// answers an error to both questions. What a caller does with "unknown"
/// is the caller's: the distiller skips the run, a dream's evidence
/// request answers "not found", and an exit interview is held.
///
/// A session a journal bound is private to a person when the journal's
/// member is, or when the stored session's own labels say so: the stricter
/// of the two. The journal answers for the binding that held the session
/// last, by the labels that binding was spawned with; they are the labels
/// the launch's record of who is private reads from the same spawn. What a
/// later build recorded of a member (a row whose labels changed) is what
/// the stored session's labels say, which that build wrote. So neither
/// alone carries the question, and the record itself is not asked.
struct MemoryTranscripts {
    of_the_store: SessionStoreTranscriptSource,
    home: TranscriptsOf,
}

/// A session a journal of the memory bound: the stored session, when the
/// store holds one, and whether it is private to a person.
struct BoundSession {
    stored: Option<meerkat_core::Session>,
    private: bool,
}

impl MemoryTranscripts {
    /// The journal that says whether the session stored as `stored` is one
    /// of this memory's own. `None`: no journal of this memory is asked for
    /// it (its stored binding names a mob whose members are not members of
    /// this memory, or a mob of this memory that is not loaded).
    async fn journal_for(
        &self,
        stored: Option<&meerkat_core::Session>,
    ) -> Result<Option<Arc<dyn BoundSessions>>, String> {
        let (launch_mob, journal, mobs) = match &self.home {
            TranscriptsOf::AHostedMob(journal) => return Ok(Some(Arc::clone(journal))),
            TranscriptsOf::TheLaunch {
                launch_mob,
                journal,
                mobs,
            } => (launch_mob, journal, mobs),
        };
        let Some(named) = stored
            .and_then(meerkat_core::Session::session_metadata)
            .and_then(|metadata| metadata.mob_member_binding)
        else {
            return Ok(Some(Arc::clone(journal)));
        };
        // Whose memory the named mob's members are in, before any journal.
        if mobs.home_of(&named.mob_id) != crate::private_owners::MemoryHome::Launch {
            return Ok(None);
        }
        if named.mob_id == launch_mob.as_str() {
            return Ok(Some(Arc::clone(journal)));
        }
        mobs.loaded_journal_of(&named.mob_id).await
    }

    /// The session `session_key` names, when a journal of this memory bound
    /// it.
    async fn bound(&self, session_key: &str) -> Result<Option<BoundSession>, DistillerError> {
        let session_id = meerkat_core::types::SessionId::parse(session_key).map_err(|err| {
            DistillerError::Transcript(format!("invalid session key '{session_key}': {err}"))
        })?;
        let stored = self.of_the_store.stored(session_key).await?;
        let unread = |err: String| {
            DistillerError::Transcript(format!(
                "whether the session '{session_key}' is one of this memory's own could not be \
                 read from its mob's journal: {err}"
            ))
        };
        let Some(journal) = self.journal_for(stored.as_ref()).await.map_err(unread)? else {
            return Ok(None);
        };
        let Some(member) = journal.bound(&session_id).await.map_err(unread)? else {
            return Ok(None);
        };
        let private = member.private
            || stored
                .as_ref()
                .is_some_and(SessionStoreTranscriptSource::stored_as_private);
        Ok(Some(BoundSession { stored, private }))
    }
}

#[async_trait::async_trait]
impl TranscriptSource for MemoryTranscripts {
    async fn read(
        &self,
        session_key: &str,
        from_index: u64,
    ) -> Result<Option<crate::memory::distiller::TranscriptSlice>, DistillerError> {
        Ok(self
            .bound(session_key)
            .await?
            .and_then(|bound| bound.stored)
            .map(|session| {
                SessionStoreTranscriptSource::slice_of(&session, session_key, from_index)
            }))
    }

    async fn private_to_a_person(&self, session_key: &str) -> Result<bool, DistillerError> {
        match self.bound(session_key).await? {
            Some(bound) => Ok(bound.private),
            None => Err(DistillerError::Transcript(format!(
                "the session '{session_key}' is not one {} bound: whether it is private to a \
                 person is unknown",
                self.home.journals()
            ))),
        }
    }
}

/// The dream of one mob a launch hosts beside its launch mob, as the
/// launch's dream occurrence and the mob's own load reach it while that
/// mob's identity plane lives: the mob's own steward, the mob's own store
/// value, and the source that tells which of the mob's members are private
/// to a person. Which mob it is of is the typed id of the mob the
/// composition was asked for, never a name a request or an agent carries.
///
/// Every copy shares one end ([`Self::end`]), which what serves the mob's
/// memory calls when it ends: an attempt made through [`Self::attempt`]
/// stops there, the end waits until none runs, and none starts after it.
#[derive(Clone)]
pub(crate) struct HostedMobDream {
    pub(crate) mob: meerkat_mob::MobId,
    pub(crate) steward: Arc<StewardEngine>,
    store: SqliteAgentMemoryStore,
    realm: String,
    private_members: Arc<dyn PrivateMemberSource>,
    /// `true` once what serves the mob's memory ended.
    ended: Arc<tokio::sync::watch::Sender<bool>>,
    /// Held by an attempt for as long as it runs.
    under_way: Arc<tokio::sync::Mutex<()>>,
}

/// How often a mob's load asks whether the record of who of the mob is
/// private to a person is read, before it gives the owed dream up to the
/// host's next dream occurrence.
const OWED_DREAM_WAIT_STEPS: u32 = 50;
/// The pause between two such asks.
const OWED_DREAM_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(200);

impl HostedMobDream {
    pub(crate) fn new(
        mob: meerkat_mob::MobId,
        steward: Arc<StewardEngine>,
        store: SqliteAgentMemoryStore,
        realm: String,
        private_members: Arc<dyn PrivateMemberSource>,
    ) -> Self {
        Self {
            mob,
            steward,
            store,
            realm,
            private_members,
            ended: Arc::new(tokio::sync::watch::Sender::new(false)),
            under_way: Arc::default(),
        }
    }

    /// Whether the mob keeps memory: its realm has a file. Asked of the
    /// store's directory, so the question makes no file. A mob that keeps
    /// none is not dreamed for: a dream's first read would make its file.
    fn keeps_memory(&self) -> bool {
        self.store
            .known_realms()
            .is_ok_and(|realms| realms.contains(&self.realm))
    }

    /// When the mob's newest dream run ended, in milliseconds since the
    /// epoch: none when no run of it ended, or its runs cannot be read. It
    /// reads the mob's realm, so it is asked of a mob that keeps memory.
    async fn newest_run_ended_at_ms(&self) -> Option<u64> {
        MemoryPanelStore::dream_runs(&self.store, &self.realm, 1)
            .await
            .ok()?
            .first()
            .map(|run| run.completed_at_ms)
            .filter(|ended_at_ms| *ended_at_ms > 0)
    }

    fn has_ended(&self) -> bool {
        *self.ended.borrow()
    }

    async fn until_ended(&self) {
        let mut ended = self.ended.subscribe();
        // The sender is this value's own: the wait ends on `true` alone.
        let _ = ended.wait_for(|ended| *ended).await;
    }

    /// No attempt starts from now on, and one under way stops.
    pub(crate) fn end_now(&self) {
        self.ended.send_replace(true);
    }

    /// [`Self::end_now`], and wait until no attempt runs.
    pub(crate) async fn end(&self) {
        self.end_now();
        drop(self.under_way.lock().await);
    }

    /// One dream attempt of the mob's steward, one at a time per mob. None
    /// is made once what serves the mob's memory ended, and none for a mob
    /// that keeps no memory; an attempt under way stops when what serves
    /// the mob's memory ends. A dream stopped so leaves what a stopped
    /// process leaves: a run row that never ended, which the store
    /// collects.
    pub(crate) async fn attempt(&self) {
        let _under_way = self.under_way.lock().await;
        if self.has_ended() || !self.keeps_memory() {
            return;
        }
        tokio::select! {
            () = self.until_ended() => {}
            _ = self.steward.dream_now() => {}
        }
    }

    /// The dream the mob is owed at its load: one attempt, when the mob
    /// keeps memory and no run of it ended within one dream cadence. The
    /// steward's own gates decide the attempt as they decide every other,
    /// so a mob with nothing to dream over is skipped.
    ///
    /// A dream runs only once the record tells which of the mob's members
    /// are private to a person, and a mob that was just loaded is read by
    /// the record a moment later: the ask waits for that, for a bounded
    /// time, and then leaves the dream to the host's next occurrence.
    /// Nothing is stored for a dream that is owed: the mob's dream runs
    /// are what tells it.
    pub(crate) async fn attempt_when_owed(&self) {
        if !self.keeps_memory() {
            return;
        }
        let mut known = false;
        for _ in 0..OWED_DREAM_WAIT_STEPS {
            if self.has_ended() {
                return;
            }
            if self.private_members.private_identities().await.is_ok() {
                known = true;
                break;
            }
            tokio::time::sleep(OWED_DREAM_WAIT_STEP).await;
        }
        if !known {
            tracing::debug!(
                mob_id = %self.mob,
                "agent memory steward: the members of a loaded mob that are private to a person \
                 are not known yet; its dream is left to the host's next dream occurrence"
            );
            return;
        }
        let cadence_ms =
            u64::try_from(self.steward.dream_cadence().as_millis()).unwrap_or(u64::MAX);
        let owed = self
            .newest_run_ended_at_ms()
            .await
            .is_none_or(|ended_at_ms| unix_ms_now().saturating_sub(ended_at_ms) >= cadence_ms);
        if owed {
            self.attempt().await;
        }
    }
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The dreams of the mobs a launch hosts beside its launch mob whose
/// identity planes live at the moment it is asked. It loads no mob and
/// keeps nothing between two asks.
pub(crate) type HostedMobDreams = Arc<dyn Fn() -> Vec<HostedMobDream> + Send + Sync>;

/// One dream occurrence of a host: a dream attempt of the launch's engine,
/// then one of the steward of each mob the launch hosts beside its launch
/// mob that is loaded and keeps memory. No mob is loaded for it, and no
/// file is made for a mob that keeps no memory.
///
/// The hosted mobs are dreamed for one after the other, the one whose
/// newest ended run is oldest first and a mob none of whose runs ended
/// before all; the mob's id settles a tie. The order is read from the
/// mobs' dream runs at the occurrence, and nothing is kept between two.
pub(crate) async fn dream_for_the_host(
    launch: &Arc<StewardEngine>,
    hosted: Option<&HostedMobDreams>,
) {
    launch.dream_now().await;
    let Some(hosted) = hosted else {
        return;
    };
    let mut order = Vec::new();
    for dream in hosted() {
        if dream.keeps_memory() {
            order.push((dream.newest_run_ended_at_ms().await, dream.mob.clone()));
        }
    }
    order.sort();
    for (_, mob) in order {
        // Asked again at each mob's turn: the mobs before it took their
        // time, and this one may have ended, or been composed anew, since.
        let Some(dream) = hosted().into_iter().find(|dream| dream.mob == mob) else {
            continue;
        };
        dream.attempt().await;
    }
}

/// The guarded interval loop of a launch with no schedule host:
/// [`dream_for_the_host`] at the launch engine's dream cadence.
pub(crate) fn spawn_host_dream_loop(
    launch: Arc<StewardEngine>,
    hosted: Option<HostedMobDreams>,
) -> tokio::task::JoinHandle<()> {
    let interval = launch.dream_cadence();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            dream_for_the_host(&launch, hosted.as_ref()).await;
        }
    })
}

/// Test builds only: the model clients a stack's engines are built with
/// in the place of the factory's.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct EngineClientsInTests {
    pub(crate) distiller: Arc<dyn crate::memory::distiller::DistillerClientHandle>,
    pub(crate) steward: Arc<dyn crate::memory::steward::StewardClientHandle>,
}

/// Test builds only: a session stored in `store` with one message, of a
/// member private to a person, whose stored member binding names the
/// member `member` of the mob `mob` when `bound_to` gives them: its key.
#[cfg(test)]
pub(crate) async fn a_stored_member_session_in_tests(
    store: &Arc<meerkat::MemoryStore>,
    bound_to: Option<(&str, &str)>,
) -> String {
    a_stored_session_in_tests(store, None, bound_to, true).await
}

/// Test builds only: a session stored in `store` with one message, whose
/// stored member binding names the member `member` of the mob `mob` when
/// `bound_to` gives them, and whose stored labels say it is the session of
/// a member private to a person when `private` says so: its key. With `id`
/// it is stored under that id, the one a mob's journal bound; without,
/// under an id of its own.
#[cfg(test)]
pub(crate) async fn a_stored_session_in_tests(
    store: &Arc<meerkat::MemoryStore>,
    id: Option<meerkat_core::types::SessionId>,
    bound_to: Option<(&str, &str)>,
    private: bool,
) -> String {
    let mut session = id.map_or_else(meerkat_core::Session::new, meerkat_core::Session::with_id);
    let stored = session.set_session_metadata(meerkat_core::SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "gpt-5.5".to_string(),
        model_fallback: None,
        max_tokens: 1024,
        structured_output_retries: meerkat_core::config::default_structured_output_retries(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: meerkat_core::SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: private.then(|| meerkat_core::PeerMeta {
            description: None,
            labels: [(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:owner".to_string(),
            )]
            .into_iter()
            .collect(),
        }),
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        mob_member_binding: bound_to.map(|(mob, member)| meerkat_core::MobMemberBinding {
            mob_id: mob.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }),
    });
    assert!(
        stored.is_ok(),
        "the session's metadata is recorded: {stored:?}"
    );
    session.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text("a marker of this session"),
    ));
    let saved = meerkat::SessionStore::save(&**store, &session).await;
    assert!(saved.is_ok(), "the session is stored: {:?}", saved.err());
    session.id().to_string()
}

/// Test builds only: what a row says the journal of one mob bound: each
/// session's key with whether the member it was bound to is private to a
/// person; or a journal that cannot be read.
#[cfg(test)]
pub(crate) struct JournalInTests(Result<std::collections::BTreeMap<String, bool>, String>);

#[cfg(test)]
impl JournalInTests {
    /// A journal that bound each session of `bound`, to a member that is
    /// private to a person where its flag says so.
    pub(crate) fn binding(bound: &[(&str, bool)]) -> Arc<dyn BoundSessions> {
        Arc::new(Self(Ok(bound
            .iter()
            .map(|(session, private)| ((*session).to_string(), *private))
            .collect())))
    }

    /// A journal whose read fails with `why`.
    pub(crate) fn unreadable(why: &str) -> Arc<dyn BoundSessions> {
        Arc::new(Self(Err(why.to_string())))
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl BoundSessions for JournalInTests {
    async fn bound(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<Option<BoundMember>, String> {
        let bound = self.0.as_ref().map_err(Clone::clone)?;
        Ok(bound
            .get(&session.to_string())
            .map(|private| BoundMember { private: *private }))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// The stack assembled over the runtime's provider credential store
    /// gives the Distiller and the Steward that one store's refresh
    /// authority, while the stock assembly keeps them on the process default.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_engines_resolve_credentials_in_the_store_they_are_handed() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let credentials = meerkat::TokenStoreBackend::File {
            root: dir.path().join("credentials"),
        }
        .open_with_refresh_authority()
        .expect("credential store");
        let engines = MemoryEnginesConfig {
            distiller: DistillerConfig {
                enabled: true,
                ..Default::default()
            },
            steward: StewardConfig {
                enabled: true,
                ..Default::default()
            },
        };
        let assemble = |name: &str, persistence: Option<meerkat::ProviderAuthPersistence>| {
            let state = dir.path().join(name);
            let provider: Arc<dyn AgentMemoryProvider> = Arc::new(
                SqliteAgentMemoryStore::open(state.join("agent-memory")).expect("memory store"),
            );
            let transcripts: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
            attach_memory_engines_with_provider_auth(
                provider,
                &AgentMemoryConfig::default(),
                &engines,
                MemoryStackSeams {
                    persistent_state: Some(state),
                    transcript_store: Some(transcripts),
                    event_sink: Some(Arc::new(crate::memory::events::CollectingEventSink::new())),
                    ..Default::default()
                },
                persistence,
            )
            .expect("the stack assembles")
        };

        let named = assemble("named", Some(credentials.clone()));
        let distiller = named.distiller.as_ref().expect("the Distiller is enabled");
        let steward = named.steward.as_ref().expect("the Steward is enabled");
        assert_eq!(
            distiller.client_provider_auth_authority(),
            Some(credentials.authority_id())
        );
        assert_eq!(
            steward.client_provider_auth_authority(),
            Some(credentials.authority_id())
        );

        let stock = assemble("stock", None);
        for authority in [
            stock
                .distiller
                .as_ref()
                .expect("the Distiller is enabled")
                .client_provider_auth_authority(),
            stock
                .steward
                .as_ref()
                .expect("the Steward is enabled")
                .client_provider_auth_authority(),
        ] {
            assert_ne!(
                authority,
                Some(credentials.authority_id()),
                "the stock assembly keeps the process default store"
            );
        }
    }

    /// A transcript source that holds no session and counts its reads.
    #[derive(Default)]
    struct CountsReads(std::sync::atomic::AtomicUsize);

    #[async_trait::async_trait]
    impl TranscriptSource for CountsReads {
        async fn read(
            &self,
            _session_key: &str,
            _from_index: u64,
        ) -> Result<
            Option<crate::memory::distiller::TranscriptSlice>,
            crate::memory::distiller::DistillerError,
        > {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
    }

    /// A stack assembled over a transcript source reads sessions through
    /// that one source alone: its Distiller and its Steward hold the very
    /// value, and the evidence resolver the assembly installs on the store
    /// asks it. The stock assembly makes a source of its own over the
    /// session store it is given.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_distiller_the_steward_and_the_evidence_resolver_read_through_the_one_source_they_are_assembled_with()
     {
        use crate::memory::records::{
            EvidenceRef, MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord, TrustTier,
            VerificationClaim,
        };
        use crate::memory::staged::{
            StagedBatchKind, StagedMemoryStore as _, StagedMutationBatch, StagedOp,
        };

        let dir = tempfile::tempdir().expect("temporary directory");
        let state = dir.path().join("state");
        let engines = MemoryEnginesConfig {
            distiller: DistillerConfig {
                enabled: true,
                ..Default::default()
            },
            steward: StewardConfig {
                enabled: true,
                ..Default::default()
            },
        };
        let config = AgentMemoryConfig::default();
        let store = Arc::new(
            SqliteAgentMemoryStore::open(state.join("agent-memory")).expect("memory store"),
        );
        let counted = Arc::new(CountsReads::default());
        let source: Arc<dyn TranscriptSource> = counted.clone();
        let stack = assemble_memory_stack(
            store.clone(),
            &config,
            &engines,
            MemoryStackSeams {
                persistent_state: Some(state),
                event_sink: Some(Arc::new(crate::memory::events::CollectingEventSink::new())),
                ..Default::default()
            },
            None,
            MemoryStackOver {
                taint: None,
                transcripts: Some(Arc::clone(&source)),
                clients: None,
            },
        )
        .expect("the stack assembles over the source, with no session store");
        let distiller = stack.distiller.as_ref().expect("the Distiller is enabled");
        let steward = stack.steward.as_ref().expect("the Steward is enabled");
        assert!(
            Arc::ptr_eq(distiller.transcripts_in_tests(), &source),
            "the Distiller reads through the source the stack was assembled over"
        );
        assert!(
            Arc::ptr_eq(steward.transcripts_in_tests(), &source),
            "the Steward reads through the source the stack was assembled over"
        );

        // The evidence resolver on the store: a raise that cites a session
        // asks the same source, which holds none.
        let scope = MemoryScope::Identity {
            realm: config.realm.clone(),
            identity: "identity:worker".to_string(),
        };
        let receipt = store
            .remember_authored(
                &scope,
                NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: "Checked fact".to_string(),
                    description: "checked against a transcript".to_string(),
                    body: "The line was seen in the session.".to_string(),
                    tags: Vec::new(),
                    evidence: Vec::new(),
                    verification: Some(VerificationClaim {
                        checked: "read the session and saw the line".to_string(),
                        evidence: vec![EvidenceRef {
                            session_id: "a-session".to_string(),
                            generation: 0,
                            revision: None,
                            range: None,
                        }],
                    }),
                },
                MemoryAuthor::Agent {
                    identity: "identity:worker".to_string(),
                },
            )
            .await
            .expect("seed");
        let before = counted.0.load(std::sync::atomic::Ordering::SeqCst);
        let staged = store
            .stage(StagedMutationBatch {
                kind: StagedBatchKind::FreshWrite,
                realm: config.realm.clone(),
                author: MemoryAuthor::Steward {
                    run_id: "dream-test".to_string(),
                },
                ops: vec![StagedOp::Retier {
                    id: receipt.memory_id,
                    trust: TrustTier::AgentVerified,
                    rationale: None,
                }],
            })
            .await;
        assert!(
            staged.is_err(),
            "a citation of a session the source does not hold does not resolve"
        );
        assert!(
            counted.0.load(std::sync::atomic::Ordering::SeqCst) > before,
            "the evidence resolver read through the source the stack was assembled over"
        );
    }

    /// The launch's other mobs, as a row says them: whose memory the
    /// members of each are in, and the journal of each that is loaded.
    #[derive(Default)]
    struct Mobs {
        homes: std::collections::BTreeMap<&'static str, crate::private_owners::MemoryHome>,
        loaded: std::collections::BTreeMap<&'static str, Arc<dyn BoundSessions>>,
        /// The mob set cannot say which mobs are loaded.
        unreadable: bool,
    }

    #[async_trait::async_trait]
    impl MobsOfTheLaunchMemory for Mobs {
        fn home_of(&self, mob_id: &str) -> crate::private_owners::MemoryHome {
            self.homes
                .get(mob_id)
                .cloned()
                .unwrap_or(crate::private_owners::MemoryHome::NotKnown)
        }

        async fn loaded_journal_of(
            &self,
            mob_id: &str,
        ) -> Result<Option<Arc<dyn BoundSessions>>, String> {
            if self.unreadable {
                return Err("the mob set is closed".to_string());
            }
            Ok(self.loaded.get(mob_id).cloned())
        }
    }

    /// What a source answers for a session: whether a read holds it (`None`
    /// when the read fails), and whether it is private to a person (`None`
    /// when the source does not say).
    async fn answered(
        source: &Arc<dyn TranscriptSource>,
        session: &str,
    ) -> (Option<bool>, Option<bool>) {
        (
            source
                .read(session, 0)
                .await
                .ok()
                .map(|read| read.is_some()),
            source.private_to_a_person(session).await.ok(),
        )
    }

    /// A session the source holds, stored with a private owner.
    const HELD: (Option<bool>, Option<bool>) = (Some(true), Some(true));
    /// A session the source does not hold: it reads as one the store does
    /// not hold, and whether it is private to a person is unknown.
    const NOT_HELD: (Option<bool>, Option<bool>) = (Some(false), None);
    /// A session whose journal cannot be read: both questions fail.
    const UNREAD: (Option<bool>, Option<bool>) = (None, None);

    /// The source a launch assembles its engines with reads the sessions of
    /// the launch's memory alone, session by session: a session is held
    /// when the journal of the mob its stored binding names bound it and
    /// that mob's members are members of the launch's memory, or, with no
    /// stored binding, when the launch mob's journal bound it. The stored
    /// binding only chooses the journal, and whose memory a mob is in is
    /// asked before any journal: a session that names a mob outside the
    /// launch's memory is not held although that mob's journal bound it.
    #[tokio::test]
    async fn the_launchs_transcript_source_reads_the_sessions_of_the_launchs_memory_alone() {
        use crate::private_owners::MemoryHome;

        let sessions = Arc::new(meerkat::MemoryStore::new());
        let stored = |bound_to: Option<(&'static str, &'static str)>| {
            let sessions = Arc::clone(&sessions);
            async move { a_stored_member_session_in_tests(&sessions, bound_to).await }
        };
        let of_the_launch_mob = stored(Some(("launch", "desk"))).await;
        let of_a_created_mob = stored(Some(("helper", "worker"))).await;
        let of_no_mob_and_bound = stored(None).await;
        let of_no_mob = stored(None).await;
        let names_the_launch_mob = stored(Some(("launch", "desk"))).await;
        let names_a_created_mob = stored(Some(("helper", "worker"))).await;
        let of_the_second_mob = stored(Some(("second", "desk"))).await;
        let of_a_mob_of_the_second = stored(Some(("of-second", "worker"))).await;
        let of_a_mob_not_loaded = stored(Some(("stored", "worker"))).await;
        let of_an_unowned_mob = stored(Some(("unowned", "worker"))).await;
        let stored_nowhere = meerkat_core::types::SessionId::new().to_string();

        // Every mob outside the launch's memory has a journal here that
        // bound its session: only whose memory the mob is in keeps it out.
        let mobs = Mobs {
            homes: [
                ("launch", MemoryHome::Launch),
                ("helper", MemoryHome::Launch),
                ("stored", MemoryHome::Launch),
                ("second", MemoryHome::Hosted("second".to_string())),
                ("of-second", MemoryHome::Nowhere),
                ("unowned", MemoryHome::NotKnown),
            ]
            .into_iter()
            .collect(),
            loaded: [
                (
                    "helper",
                    JournalInTests::binding(&[(of_a_created_mob.as_str(), false)]),
                ),
                (
                    "second",
                    JournalInTests::binding(&[(of_the_second_mob.as_str(), false)]),
                ),
                (
                    "of-second",
                    JournalInTests::binding(&[(of_a_mob_of_the_second.as_str(), false)]),
                ),
                (
                    "unowned",
                    JournalInTests::binding(&[(of_an_unowned_mob.as_str(), false)]),
                ),
            ]
            .into_iter()
            .collect(),
            unreadable: false,
        };
        // The launch mob's journal also bound the session that names a
        // created mob and the one that names a mob that is not loaded: the
        // stored binding chooses the journal, so neither is held.
        let source = launch_transcript_source_over(
            sessions.clone(),
            meerkat_mob::MobId::from("launch"),
            JournalInTests::binding(&[
                (of_the_launch_mob.as_str(), false),
                (of_no_mob_and_bound.as_str(), false),
                (names_a_created_mob.as_str(), false),
                (of_a_mob_not_loaded.as_str(), false),
            ]),
            Arc::new(mobs),
        );
        let table = [
            (&of_the_launch_mob, HELD, "the launch mob's, by its journal"),
            (
                &of_a_created_mob,
                HELD,
                "a mob's a launch mob's member created, by that mob's journal",
            ),
            (
                &of_no_mob_and_bound,
                HELD,
                "one with no stored binding that the launch mob's journal bound",
            ),
            (
                &of_no_mob,
                NOT_HELD,
                "one with no stored binding that no journal bound",
            ),
            (
                &names_the_launch_mob,
                NOT_HELD,
                "one that names the launch mob and that its journal never bound",
            ),
            (
                &names_a_created_mob,
                NOT_HELD,
                "one that names a created mob, bound by the launch mob's journal alone",
            ),
            (
                &of_the_second_mob,
                NOT_HELD,
                "the second mob's, bound by the second mob's journal",
            ),
            (
                &of_a_mob_of_the_second,
                NOT_HELD,
                "a mob's the second mob's member created, bound by that mob's journal",
            ),
            (
                &of_a_mob_not_loaded,
                NOT_HELD,
                "a mob's of the launch's memory that is not loaded",
            ),
            (
                &of_an_unowned_mob,
                NOT_HELD,
                "a mob's no member created, bound by that mob's journal",
            ),
            (
                &stored_nowhere,
                NOT_HELD,
                "an id the store holds none under",
            ),
        ];
        let mut answers = Vec::new();
        let mut wished = Vec::new();
        for (session, expected, which) in table {
            answers.push((which, answered(&source, session).await));
            wished.push((which, expected));
        }
        assert_eq!(
            answers, wished,
            "what the source answers for each session (left), and what it is to answer (right)"
        );
    }

    /// A journal that cannot be read answers an error to both questions,
    /// never "not held": the launch mob's journal, a created mob's, and a
    /// mob set that cannot say which mobs are loaded. A session that names
    /// a mob outside the launch's memory is answered before any journal is
    /// asked, so it is not held whatever the journals do.
    #[tokio::test]
    async fn the_launchs_transcript_source_answers_an_error_when_a_journal_it_asks_cannot_be_read()
    {
        use crate::private_owners::MemoryHome;

        let sessions = Arc::new(meerkat::MemoryStore::new());
        let of_the_launch_mob =
            a_stored_member_session_in_tests(&sessions, Some(("launch", "desk"))).await;
        let of_no_mob = a_stored_member_session_in_tests(&sessions, None).await;
        let of_a_created_mob =
            a_stored_member_session_in_tests(&sessions, Some(("helper", "worker"))).await;
        let of_the_second_mob =
            a_stored_member_session_in_tests(&sessions, Some(("second", "desk"))).await;
        let homes = || -> std::collections::BTreeMap<&'static str, MemoryHome> {
            [
                ("launch", MemoryHome::Launch),
                ("helper", MemoryHome::Launch),
                ("second", MemoryHome::Hosted("second".to_string())),
            ]
            .into_iter()
            .collect()
        };
        let over = |journal: Arc<dyn BoundSessions>, mobs: Mobs| {
            launch_transcript_source_over(
                sessions.clone(),
                meerkat_mob::MobId::from("launch"),
                journal,
                Arc::new(mobs),
            )
        };

        // The launch mob's journal cannot be read.
        let source = over(
            JournalInTests::unreadable("the store is closed"),
            Mobs {
                homes: homes(),
                ..Mobs::default()
            },
        );
        for (session, expected, which) in [
            (&of_the_launch_mob, UNREAD, "names the launch mob"),
            (&of_no_mob, UNREAD, "names no mob"),
            (&of_the_second_mob, NOT_HELD, "names the second mob"),
        ] {
            assert_eq!(
                answered(&source, session).await,
                expected,
                "the launch mob's journal cannot be read: a session that {which}"
            );
        }
        let said = source
            .private_to_a_person(&of_the_launch_mob)
            .await
            .expect_err("no answer");
        assert!(
            said.to_string().contains("the store is closed"),
            "the answer carries why the journal could not be read: {said}"
        );

        // A created mob's journal cannot be read.
        let source = over(
            JournalInTests::binding(&[(of_the_launch_mob.as_str(), false)]),
            Mobs {
                homes: homes(),
                loaded: [("helper", JournalInTests::unreadable("the store is closed"))]
                    .into_iter()
                    .collect(),
                unreadable: false,
            },
        );
        for (session, expected, which) in [
            (&of_a_created_mob, UNREAD, "names the created mob"),
            (&of_the_launch_mob, HELD, "names the launch mob"),
        ] {
            assert_eq!(
                answered(&source, session).await,
                expected,
                "a created mob's journal cannot be read: a session that {which}"
            );
        }

        // The mob set cannot say which mobs are loaded.
        let source = over(
            JournalInTests::binding(&[(of_the_launch_mob.as_str(), false)]),
            Mobs {
                homes: homes(),
                unreadable: true,
                ..Mobs::default()
            },
        );
        for (session, expected, which) in [
            (&of_a_created_mob, UNREAD, "names the created mob"),
            (&of_the_launch_mob, HELD, "names the launch mob"),
            (&of_the_second_mob, NOT_HELD, "names the second mob"),
        ] {
            assert_eq!(
                answered(&source, session).await,
                expected,
                "the mob set cannot be asked: a session that {which}"
            );
        }
    }

    /// A session a journal bound is private to a person when the journal's
    /// member is or when its stored labels say so, on both homes: what the
    /// journal says of the member decides alone for a session stored with
    /// no private owner, and a stored private owner stands whatever the
    /// journal says. A bound session the store holds none of is not read,
    /// and is private as the journal's member is.
    #[tokio::test]
    async fn a_bound_session_is_private_when_the_journals_member_or_its_stored_labels_say_so() {
        let sessions = Arc::new(meerkat::MemoryStore::new());
        let shared = |private: bool| {
            let sessions = Arc::clone(&sessions);
            async move {
                a_stored_session_in_tests(&sessions, None, Some(("launch", "desk")), private).await
            }
        };
        let private_by_the_journal = shared(false).await;
        let private_by_its_labels = shared(true).await;
        let private_by_both = shared(true).await;
        let private_to_no_one = shared(false).await;
        let gone_and_private = meerkat_core::types::SessionId::new().to_string();
        let gone_and_shared = meerkat_core::types::SessionId::new().to_string();
        let journal = || {
            JournalInTests::binding(&[
                (private_by_the_journal.as_str(), true),
                (private_by_its_labels.as_str(), false),
                (private_by_both.as_str(), true),
                (private_to_no_one.as_str(), false),
                (gone_and_private.as_str(), true),
                (gone_and_shared.as_str(), false),
            ])
        };
        let of_the_launch = launch_transcript_source_over(
            sessions.clone(),
            meerkat_mob::MobId::from("launch"),
            journal(),
            Arc::new(crate::private_owners::PrivateOwners::default()),
        );
        let of_a_hosted_mob = hosted_mob_transcript_source_over(sessions.clone(), journal());
        for (source, home) in [
            (&of_the_launch, "the launch"),
            (&of_a_hosted_mob, "a hosted mob"),
        ] {
            for (session, expected, which) in [
                (&private_by_the_journal, (Some(true), Some(true)), "journal"),
                (&private_by_its_labels, (Some(true), Some(true)), "labels"),
                (&private_by_both, (Some(true), Some(true)), "both"),
                (&private_to_no_one, (Some(true), Some(false)), "neither"),
                (
                    &gone_and_private,
                    (Some(false), Some(true)),
                    "journal, no stored session",
                ),
                (
                    &gone_and_shared,
                    (Some(false), Some(false)),
                    "neither, no stored session",
                ),
            ] {
                assert_eq!(
                    answered(source, session).await,
                    expected,
                    "{home}: a session private by: {which}"
                );
            }
        }
    }

    /// A launch whose record does not name its mob its own is told so
    /// before it assembles its engines.
    #[test]
    fn a_launch_whose_record_does_not_name_its_mob_is_refused_its_engines() {
        let launch_mob = meerkat_mob::MobId::from("launch");
        let owners = crate::private_owners::PrivateOwners::default();
        owners.bind_launch_mob("launch");
        assert_eq!(launch_mob_is_bound(&owners, &launch_mob), Ok(()));

        // A record that follows a mob set and was told no launch mob.
        let unbound = crate::private_owners::PrivateOwners::default();
        unbound.follow_a_mob_set_that_has_gone_in_tests();
        let refused = launch_mob_is_bound(&unbound, &launch_mob);
        assert!(
            refused
                .as_ref()
                .is_err_and(|reason| reason.contains("launch") && reason.contains("NotKnown")),
            "{refused:?}"
        );
    }
}
