//! Shared gateway construction seams (doctrine D2: converge construction,
//! merge binaries later).
//!
//! The K1/K2 field failures came from the two gateway binaries embedding the
//! same `UnifiedRuntime` with DIVERGENT wiring — `rpc_gateway` built the full
//! identity substrate, `meta_gateway` built none. Every piece both binaries
//! must construct identically belongs here so the drift class is structural
//! history. The full store construction converges with the 0.8 binary merge;
//! v1 shares the identity substrate, whose divergence caused the field
//! failures.
//!
//! The identity session bridge ([`identity_session_bridge`]) is shared by
//! both binaries and by `UnifiedRuntimeBuilder`: three compositions, one
//! construction. The member build wiring (`MemberBuildWiring`: the
//! customizer tool registry, its spawn customizer and the member build
//! preparer) is shared the same way by `UnifiedRuntimeBuilder`, the stdio
//! gateway and `UnifiedRuntime::bootstrap`.

use std::path::Path;
use std::sync::Arc;

use crate::identity_first::{
    AgentCustomizer, AgentRuntimeServices, ContinuityStore, CustomizerToolRegistry,
    CustomizerToolsSpawnCustomizer, DurabilityPolicy, IdentityBootstrapMode, IdentityBuildPreparer,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError,
    LocalContinuityStore, LocalLeaseProvider, RosterProvider, SessionRepairScope, TopologyProvider,
    contracts::LeaseProvider,
};

/// The durable-identity substrate a gateway builds from its state directory:
/// the continuity store and a lease provider whose fencing counter resumes
/// ABOVE the persisted high-water, so a restart with existing continuity
/// history never presents a stale token.
pub struct GatewayIdentitySubstrate {
    pub continuity_store: Arc<dyn ContinuityStore>,
    pub lease_provider: Arc<dyn LeaseProvider>,
}

/// Open the local identity substrate at `continuity_db`.
///
/// Fails loudly rather than degrading: a 0 fencing floor on an existing
/// store would re-arm the restart-abort class the floor seeding prevents.
pub async fn open_identity_substrate(
    continuity_db: &Path,
) -> Result<GatewayIdentitySubstrate, String> {
    let (store, fencing_floor) =
        LocalContinuityStore::open_with_fencing_floor(continuity_db.to_path_buf())
            .await
            .map_err(|e| {
                format!(
                    "failed to open continuity store and read fencing high-water at {}: {e}",
                    continuity_db.display()
                )
            })?;
    Ok(GatewayIdentitySubstrate {
        continuity_store: Arc::new(store),
        lease_provider: Arc::new(LocalLeaseProvider::with_floor(fencing_floor)),
    })
}

/// The mob handle a gateway gives the identity runtime it installs, for its
/// services and session bridge. The installed identity runtime lives inside
/// the composition the public owner keeps alive, so this is a back-edge and
/// does not own the mob (D67), as in the builder: an owning handle there
/// would keep admission open after the last external owner drops, and the
/// runtime's retained teardown would wait on that close indefinitely.
#[must_use]
pub fn identity_mob_handle(runtime: &crate::UnifiedRuntime) -> meerkat_mob::MobHandle {
    runtime.mob_handle().observer()
}

/// Keep `identity_runtime`'s identity records (each build's tool record,
/// each rotation's intent, and the lifecycle projection's cursor and the
/// side effects it owes) in `runtime`'s metadata store, keyed by its mob: the
/// store the stdio gateway and `UnifiedRuntimeBuilder` give their identity
/// runtime. On a launch whose metadata store is its state directory's, a
/// restart keeps them; without this call they are kept for the process only.
///
/// Call it once, with the runtime `identity_runtime` serves, right after the
/// identity runtime is built and before its identity context is installed
/// (`UnifiedRuntime::install_and_bootstrap_identity_first_context`): the
/// records written before the call stay in the identity runtime's own
/// store, for the process only, and nothing reads them after it.
pub fn keep_identity_records_in_runtime_metadata(
    identity_runtime: &IdentityRuntime,
    runtime: &crate::UnifiedRuntime,
) {
    identity_runtime
        .set_identity_records(Arc::clone(runtime.persistent_metadata()), runtime.mob_id());
}

/// The identity-first session bridge a composition installs: Meta's bridge
/// over `handle`, with every seam `mob_runtime` recorded at its bootstrap.
/// `UnifiedRuntimeBuilder` and both gateway binaries build their identity
/// bridge here, so none of them can leave a seam out. The builder once built
/// its bridge without the controller pin source, and every governed send on
/// its runtime was refused after Meta had woken and built the member.
#[must_use]
pub fn identity_session_bridge(
    mob_runtime: &crate::mob_handle_runtime::MobRuntime,
    handle: meerkat_mob::MobHandle,
) -> crate::identity_first::MobSessionBridge {
    let mut bridge = match mob_runtime.session_service().cloned() {
        Some(service) => {
            crate::identity_first::MobSessionBridge::with_session_service(handle, service)
        }
        None => crate::identity_first::MobSessionBridge::new(handle),
    };
    // Heal seam (2026-07-29 incident): the continuity repair supervisor asks
    // this recoverer to commit the durable head before declaring an identity
    // healed; without it, heal is a cosmetic entry reset that the next
    // materialization re-Breaks.
    if let Some(recoverer) = mob_runtime.committed_boundary_recoverer() {
        bridge = bridge.with_committed_boundary_recoverer(recoverer);
    }
    // Image deliveries are judged with the member builder's host config, so a
    // self-hosted model's `vision` flag counts.
    if let Some(config) = mob_runtime.member_model_config() {
        bridge = bridge.with_member_model_config(config);
    }
    // Governed binding pins the member session actor's actual controller
    // through the concrete session owner, and a live MCP attachment reaches
    // the current member through it.
    if let Some(source) = mob_runtime.controller_pin_source() {
        bridge = bridge.with_controller_pin_source(source);
    }
    bridge
}

/// Makes the customizer of one mob's agents from the runtime services of
/// that mob's identity runtime
/// ([`HostMobIdentities::with_customizer_of_each_mob`]). `Ok(None)`: that
/// mob's agents have no customizer. `Err`: nothing is composed for the
/// mob, and the composition is asked again.
pub type CustomizerOfAMob = Arc<
    dyn Fn(&AgentRuntimeServices) -> Result<Option<Arc<dyn AgentCustomizer>>, String> + Send + Sync,
>;

/// How a launch composes the identity plane of each mob it hosts beside its
/// launch mob: the values the launch gave its launch mob's identity
/// runtime that hold for every mob of the host, kept once, and handed to
/// the runtime ([`Self::hand_over_to`]). Both gateway binaries and
/// `UnifiedRuntimeBuilder` build it here, after their launch mob's identity
/// bootstrap.
///
/// What it composes for one mob is what the launch composes for its launch
/// mob, over THAT mob's own handle: the session bridge
/// ([`identity_session_bridge`]), an identity runtime with the mob's
/// runtime services, and the identity context with the roster provider
/// and the topology provider this was given. Every launch hands its roster
/// over as its launch mob's own and hands no topology provider over: a mob
/// hosted beside the launch mob is composed with no agent of the launch's
/// roster and with no topology provider, and both providers are asked for
/// the launch mob alone.
///
/// What it does not carry, because the launch binds it to its launch mob:
/// the first-boot manifest and the held-completion marks of the state
/// directory, and a native host's own hooks.
///
/// Agent memory is kept per mob. A launch that keeps agent memory in the
/// bundled store hands that memory over
/// ([`Self::with_agent_memory_per_mob`]) with its customizer as it was
/// before the launch wrapped it with its launch mob's memory: each mob
/// hosted beside the launch mob is then composed with a memory of its own
/// ([`HostMobMemory`]). A launch whose agent memory cannot be kept per mob
/// (a host's own memory provider, a native host that composes the
/// customizer itself) says so, and hands nothing over
/// ([`Self::with_agent_memory_of_the_launch_mob`]).
#[non_exhaustive]
pub struct HostMobIdentities {
    /// The launch's roster provider: one for every mob of the host, asked
    /// with the definition of the mob it answers for. Both gateways and the
    /// runtime builder hand theirs over as their launch mob's own
    /// ([`Self::with_roster_of_the_launch_mob_alone`]): it is then asked
    /// for the launch mob alone.
    pub(crate) roster: Arc<dyn RosterProvider>,
    pub(crate) topology: Option<Arc<dyn TopologyProvider>>,
    /// The customizer of those mobs' agents: the launch's one composed
    /// customizer, the application's and the host's own, WITHOUT the
    /// launch mob's agent memory. It serves every mob where it reads the
    /// mob from each build's own context.
    pub(crate) customizer: Option<Arc<dyn AgentCustomizer>>,
    /// How the customizer of one mob's agents is made from that mob's
    /// runtime services ([`Self::with_customizer_of_each_mob`]). Set, it
    /// stands in place of `customizer` for every mob composed here.
    pub(crate) customizer_of_each_mob: Option<CustomizerOfAMob>,
    pub(crate) error_hook: Option<crate::unified_runtime::ErrorHook>,
    /// Whether the launch has a durable runtime store.
    pub(crate) has_runtime_store: bool,
    pub(crate) bootstrap_mode: IdentityBootstrapMode,
    /// Whether this is a candidate launch, which releases no member's run.
    pub(crate) candidate_launch: bool,
    pub(crate) session_repair_scope: Option<SessionRepairScope>,
    pub(crate) transcript_edit_service:
        Option<Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>>,
    /// Whether the launch keeps agent memory that is its launch mob's alone
    /// ([`Self::with_agent_memory_of_the_launch_mob`]).
    pub(crate) agent_memory_of_the_launch_mob: bool,
    /// The agent memory the launch keeps in the bundled store, for each mob
    /// it hosts beside its launch mob ([`Self::with_agent_memory_per_mob`]).
    pub(crate) agent_memory: Option<HostMobMemory>,
}

/// The agent memory a launch keeps in the bundled store, as the launch
/// hands it over for the mobs it hosts beside its launch mob: the launch's
/// own store, its agent memory configuration and the id of its launch mob.
///
/// Each such mob keeps its agents' memory in a store of its own, a
/// directory of that mob below the launch's memory root
/// (`SqliteAgentMemoryStore::of_hosted_mob`), under the mob's own memory
/// realm ([`AgentMemoryConfig::for_mob`]). Which mob a memory is kept for
/// is the mob the launch's mob set built the entry for, never a name a
/// request or an agent carries: the composer derives both from the typed
/// id of the mob it is asked for, and hands the mob's store to that mob's
/// owners alone. The launch mob's memory stays where it is, in the
/// launch's own root.
///
/// [`AgentMemoryConfig::for_mob`]: crate::identity_first::agent_memory::AgentMemoryConfig::for_mob
pub struct HostMobMemory {
    pub(crate) store: crate::memory::sqlite_store::SqliteAgentMemoryStore,
    pub(crate) config: crate::identity_first::agent_memory::AgentMemoryConfig,
    pub(crate) launch_mob: meerkat_mob::MobId,
    /// The host's explicit answer, per profile name, whether an agent of
    /// that profile gets agent memory.
    pub(crate) profile_memory_policy: std::collections::BTreeMap<meerkat_mob::ProfileName, bool>,
    /// The launch's two memory slots, which every member build of the
    /// launch's mobs passes: each mob's memory is entered in them, so a
    /// member of that mob is observed by, and bound to, its own mob's
    /// memory. Set at the hand-over ([`HostMobIdentities::hand_over_to`]).
    pub(crate) slots: Option<MemorySlots>,
    /// The profiles the launch listed their own memory only, checked
    /// against the launch mob's definition. They hold by name in every mob
    /// of the host: an agent of a profile of that name is listed its own
    /// memory only, and each mob's agents are judged by that mob's own
    /// records.
    pub(crate) own_memory_only_profiles: std::collections::BTreeSet<meerkat_mob::ProfileName>,
    /// What the memory engines of those mobs are built with, where the
    /// launch runs memory engines ([`Self::with_engines`]).
    pub(crate) engines: Option<HostMobEngines>,
}

/// What a launch hands over for the memory engines (the distiller and the
/// steward) of the mobs it hosts beside its launch mob: the engines its
/// configuration turns on, the state directory their model clients are
/// resolved under, the session store the launch's mobs persist their
/// sessions to, and the provider credential store the launch's agents use.
pub struct HostMobEngines {
    pub(crate) config: crate::memory_wiring::MemoryEnginesConfig,
    pub(crate) persistent_state: std::path::PathBuf,
    pub(crate) sessions: Arc<dyn meerkat::SessionStore>,
    pub(crate) provider_auth: Option<meerkat::ProviderAuthPersistence>,
    /// Test builds only: the model clients the engines of every such mob
    /// are built with.
    #[cfg(test)]
    pub(crate) clients: Option<crate::memory_wiring::EngineClientsInTests>,
}

impl HostMobEngines {
    /// The engines `config` turns on, over the state directory
    /// `persistent_state` and the session store `sessions`.
    #[must_use]
    pub fn new(
        config: crate::memory_wiring::MemoryEnginesConfig,
        persistent_state: std::path::PathBuf,
        sessions: Arc<dyn meerkat::SessionStore>,
    ) -> Self {
        Self {
            config,
            persistent_state,
            sessions,
            provider_auth: None,
            #[cfg(test)]
            clients: None,
        }
    }

    /// The provider credential store the engines resolve their credentials
    /// in. `None`: the process default store.
    #[must_use]
    pub fn with_provider_auth(
        mut self,
        provider_auth: Option<meerkat::ProviderAuthPersistence>,
    ) -> Self {
        self.provider_auth = provider_auth;
        self
    }

    /// Test builds only: the model clients the engines are built with.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_clients_in_tests(
        mut self,
        clients: crate::memory_wiring::EngineClientsInTests,
    ) -> Self {
        self.clients = Some(clients);
        self
    }
}

/// The two memory slots of a launch
/// ([`crate::mob_handle_runtime::MobRuntime::dispatch_taint_slot`],
/// [`crate::mob_handle_runtime::MobRuntime::side_chat_memory_slot`]).
#[derive(Clone)]
pub(crate) struct MemorySlots {
    pub(crate) taint: crate::memory::dispatch_taint::DispatchTaintSlot,
    pub(crate) side_chat: crate::memory::side_chat_memory::SideChatMemorySlot,
}

impl HostMobMemory {
    /// The launch's bundled store `store`, the configuration `config` its
    /// launch mob's agent memory is built with, and the id of its launch
    /// mob.
    #[must_use]
    pub fn new(
        store: crate::memory::sqlite_store::SqliteAgentMemoryStore,
        config: crate::identity_first::agent_memory::AgentMemoryConfig,
        launch_mob: meerkat_mob::MobId,
    ) -> Self {
        Self {
            store,
            config,
            launch_mob,
            profile_memory_policy: std::collections::BTreeMap::new(),
            slots: None,
            own_memory_only_profiles: std::collections::BTreeSet::new(),
            engines: None,
        }
    }

    /// The memory engines of the mobs the launch hosts beside its launch
    /// mob: each such mob gets a distiller and a steward of its own, over
    /// its own memory, where `engines` turns them on. `None`: those mobs'
    /// memory runs no engine.
    #[must_use]
    pub fn with_engines(mut self, engines: Option<HostMobEngines>) -> Self {
        self.engines = engines;
        self
    }

    /// The host's explicit answer, per profile name, whether an agent of
    /// that profile gets agent memory: it holds for a profile of that name
    /// in every mob of the host.
    #[must_use]
    pub fn with_profile_memory_policy(
        mut self,
        policy: std::collections::BTreeMap<meerkat_mob::ProfileName, bool>,
    ) -> Self {
        self.profile_memory_policy = policy;
        self
    }

    /// The profiles the launch listed their own memory only
    /// (`own_memory_only_profiles`), as its start checked them against the
    /// launch mob's definition: a mob hosted beside the launch mob gives an
    /// agent of a profile of one of these names, and an agent such an agent
    /// created there, its own memory only. Who created an agent of a mob is
    /// read from that mob's own records.
    #[must_use]
    pub fn with_own_memory_only_profiles(
        mut self,
        profiles: std::collections::BTreeSet<meerkat_mob::ProfileName>,
    ) -> Self {
        self.own_memory_only_profiles = profiles;
        self
    }

    /// The first ask of a composition for the mob `mob`, a mob hosted beside
    /// the launch mob: the mob's own memory configuration and its own
    /// store, made for this mob at its own incarnation, read from the
    /// mob's handle (its durable record). The directory under the mob's
    /// name is asked whose it is before anything is built over it: a
    /// directory that is not proved the mob's is a refusal, not a mob
    /// without memory, and the composition answers it and composes nothing.
    ///
    /// Nothing else of the mob's memory is made here: its stack, its
    /// engines, the source of the transcripts they read and its dream are
    /// made by [`Self::of_mob`], over what this answered, so none of them
    /// exists for a mob this refused. `None`, logged, when the mob can keep
    /// no memory (its id has no memory realm): the mob is then composed
    /// without agent memory.
    fn store_of_mob(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
    ) -> Result<Option<ProvedMobStore>, crate::memory::sqlite_store::HostedMemoryRefusal> {
        let mob_id = mob.mob_id();
        let incarnation = mob.handle().incarnation();
        let default_mob = crate::member_key::DefaultMob::new(self.launch_mob.clone());
        let config_and_store = self
            .config
            .for_mob(&default_mob, mob_id)
            .and_then(|config| Ok((config, self.store.of_hosted_mob(mob_id, incarnation)?)));
        if let Ok((_, store)) = &config_and_store {
            store.proved_its_own()?;
        }
        match config_and_store {
            Ok((config, store)) => Ok(Some(ProvedMobStore { config, store })),
            Err(error) => {
                tracing::warn!(
                    %mob_id,
                    %error,
                    "this mob of the host can keep no agent memory; its identity plane is \
                     composed without one"
                );
                Ok(None)
            }
        }
    }

    /// The memory of the mob `mob` over `proved`, its configuration and its
    /// store as the composition's first ask proved them the mob's own
    /// ([`Self::store_of_mob`]): the store with the write firewall the
    /// launch's has, a sink that files its memory events in
    /// `console_events`, the console event store the mob's entry holds,
    /// and, where the launch runs memory engines, the mob's own distiller
    /// and steward, the source of the transcripts they read and the mob's
    /// dream. It is asked once the mob's identity runtime `identities` is
    /// made, which the mob's steward reads its mob through.
    ///
    /// The console event store is the entry's, handed in: none is asked of
    /// the host by the mob's id, so what is filed for an entry that ended
    /// never reaches the store a later mob of the id is given. `None`
    /// there: the mob's memory events are filed nowhere.
    ///
    /// `None`, logged, when the stack cannot be assembled: the mob is then
    /// composed without agent memory.
    fn of_mob(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
        proved: ProvedMobStore,
        identities: &Arc<IdentityRuntime>,
        console_events: Option<crate::unified_runtime::ConsoleEventStore>,
    ) -> Option<MobMemory> {
        let mob_id = mob.mob_id();
        let ProvedMobStore { config, store } = proved;
        let built: Result<MobMemory, String> = {
            let event_sink: Arc<dyn crate::memory::events::MemoryEventSink> = match console_events {
                Some(of_the_entry) => {
                    crate::unified_runtime::console_memory_event_sink(of_the_entry)
                }
                None => Arc::new(NoMemoryEvents),
            };
            // The mob's own stack: the store's write firewall over this
            // mob's tracker. Where the launch's slots are known, the
            // tracker is the one the launch's taint slot keeps for the
            // mob for the life of the process: the model calls of the
            // mob's members mark the tracker its store's write gate
            // reads, and a mark outlasts the mob's unload and load, as
            // the launch mob's marks do.
            // The store's steward surface is the provider's own value:
            // what holds the mob's memory (its identity plane) keeps it
            // alive, and what reads it weakly ends with the mob's
            // memory.
            //
            // The mob's own engines, where the launch runs memory
            // engines: a distiller and a steward over this mob's
            // store and realm alone. They read the transcripts of
            // this mob's own sessions and no other mob's; the steward
            // knows one mob, this one, so what it promotes becomes
            // this mob's shared memory; and the members of this mob
            // private to a person are read from this mob's own
            // record. A hosted mob's steward is given no way to put a
            // promotion before a person and none to file a conflict
            // with the host: the host's approvals and its ledger are
            // one for all its mobs, and what a hosted mob's agents
            // wrote is not shown there.
            let engines = self.engines.as_ref();
            let of_the_mob = store.clone();
            let realm = config.realm.clone();
            let store = Arc::new(store);
            let steward_store: Arc<dyn crate::memory::capabilities::StewardStore> = store.clone();
            // Who of this mob is private to a person, as this mob's own
            // record tells: the mob's steward dreams over none of them,
            // and the dream the mob is owed at its load waits until the
            // record answers.
            let private_members = engines.map(|_| mob.private_member_source());
            crate::memory_wiring::assemble_memory_stack(
                store,
                &config,
                &engines
                    .map(|engines| engines.config.clone())
                    .unwrap_or_default(),
                crate::memory_wiring::MemoryStackSeams {
                    persistent_state: engines.map(|engines| engines.persistent_state.clone()),
                    event_sink: Some(event_sink),
                    mob_purpose: engines.map(|_| {
                        Arc::new(HostedMobPurpose {
                            mob: mob_id.to_string(),
                            identities: Arc::downgrade(identities),
                        })
                            as Arc<dyn crate::memory::steward::MobPurposeSource>
                    }),
                    private_members: private_members.clone(),
                    ..Default::default()
                },
                engines.and_then(|engines| engines.provider_auth.clone()),
                crate::memory_wiring::MemoryStackOver {
                    taint: self
                        .slots
                        .as_ref()
                        .and_then(|slots| slots.taint.tracker_of_hosted_mob(mob_id.as_str())),
                    transcripts: engines.map(|engines| {
                        crate::memory_wiring::hosted_mob_transcript_source(
                            Arc::clone(&engines.sessions),
                            mob.handle(),
                        )
                    }),
                    #[cfg(test)]
                    clients: engines.and_then(|engines| engines.clients.clone()),
                },
            )
            .map(|stack| MobMemory {
                provider: stack.provider,
                config,
                taint: stack.taint,
                sinks: stack.sinks,
                steward_store,
                distiller: stack.distiller,
                dream: stack
                    .steward
                    .zip(private_members)
                    .map(|(steward, private_members)| {
                        crate::memory_wiring::HostedMobDream::new(
                            mob_id.clone(),
                            steward,
                            of_the_mob,
                            realm,
                            private_members,
                        )
                    }),
            })
        };
        match built {
            Ok(memory) => Some(memory),
            Err(error) => {
                tracing::warn!(
                    %mob_id,
                    %error,
                    "this mob of the host can keep no agent memory; its identity plane is \
                     composed without one"
                );
                None
            }
        }
    }

    /// Have the mob `mob` served by its own memory `memory`, at this
    /// composition of it: what a gateway does for its launch mob's memory
    /// where it assembles it.
    ///
    /// - The private record reads the private retirements the mob's store
    ///   recorded: they name members of this mob's memory alone.
    /// - The mob is entered in the launch's side chat slot: a side chat of
    ///   one of its agents, and a member of it private to a person, are
    ///   bound to this mob's memory, while the mob's identity runtime
    ///   lives.
    /// - The mob's tracker declares a member's outbound taint through this
    ///   mob's handle.
    /// - The mob's member events are observed for its tracker, and a
    ///   compaction of a member's session resets what `injector` has
    ///   delivered to it.
    ///
    /// What holds the mob for that (the observer, the handle the tracker
    /// declares through) is returned: the entry that holds the mob's
    /// identity plane keeps it and ends it with the plane
    /// ([`ServedMobMemory`]).
    fn serve(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
        handle: &meerkat_mob::MobHandle,
        bridge: Option<&Arc<dyn crate::identity_first::SessionBridge>>,
        memory: &MobMemory,
        injector: &crate::identity_first::AgentMemoryRuntimeInjector,
    ) -> ServedMobMemory {
        // After the composition named the mob (the mob set built this
        // runtime through it) and before the mob's identity plane is
        // attached, so before a request for the mob is served.
        mob.bind_private_retirements(
            Arc::clone(&memory.steward_store),
            memory.config.realm.clone(),
        );
        if let (Some(slots), Some(bridge)) = (self.slots.as_ref(), bridge) {
            slots.side_chat.register_hosted_mob(
                mob.mob_id().as_str(),
                &memory.provider,
                memory.config.clone(),
                crate::memory::side_chat_memory::SideChatMemoryGate::AgentProfiles(
                    self.profile_memory_policy.clone(),
                ),
                handle.definition(),
                bridge,
            );
        }
        // The tracker is kept for the life of the process, the mob is not:
        // the tracker's declarer holds the mob's handle weakly, and what
        // this composition serves the mob's memory with holds it. Once
        // that ended, a declaration finds no mob and declares nothing; the
        // mob's next composition sets a declarer over its own handle.
        let declares_through = Arc::new(handle.clone());
        let held_by_the_composition = Arc::downgrade(&declares_through);
        memory.taint.set_outbound_taint_declarer(Arc::new(
            move |identity: &str, taint: Option<meerkat_core::comms::SenderContentTaint>| {
                let Some(handle) = held_by_the_composition.upgrade() else {
                    return;
                };
                let member = crate::member_comms_id::mob_member_id(identity);
                let identity = identity.to_string();
                tokio::spawn(async move {
                    if let Err(error) = handle.declare_member_outbound_taint(member, taint).await {
                        tracing::debug!(
                            %identity,
                            ?taint,
                            %error,
                            "agent memory taint: outbound taint declaration failed (member may \
                             not be materialized)"
                        );
                    }
                });
            },
        ));
        let mut sinks = memory.sinks.clone();
        let resets = injector.clone();
        sinks.push(Arc::new(crate::CompactionResetSink::new(Arc::new(
            move |session: &str| resets.on_session_compacted(session),
        ))));
        // The observer holds the mob's handle, the sinks of the mob's
        // memory and its injector: it is the composition's, and ends with
        // the mob's identity plane. So is the mob's dream: its steward and
        // its store value are reached by the host's dream occurrence and
        // by the mob's own load only while this lives. And so are the
        // detached runs of the mob's distiller.
        ServedMobMemory {
            observer: crate::spawn_member_event_observer(handle.clone(), sinks),
            declares_through,
            dream: memory
                .dream
                .clone()
                .map(|dream| ServedDream { dream, owed: None }),
            distiller: memory.distiller.clone().map(ServedDistiller),
        }
    }
}

/// The dream of a hosted mob as what serves the mob's memory holds it: the
/// dream, whose copies the host's dream occurrence takes for one attempt,
/// and the task that asks the dream the mob is owed at its load. Both end
/// with it: ended ([`Self::end`]) it stops what is under way and waits for
/// it, and dropped it stops it.
struct ServedDream {
    dream: crate::memory_wiring::HostedMobDream,
    owed: Option<tokio::task::JoinHandle<()>>,
}

impl ServedDream {
    async fn end(mut self) {
        self.dream.end_now();
        if let Some(owed) = self.owed.take() {
            owed.abort();
            let _ = owed.await;
        }
        self.dream.end().await;
    }
}

impl Drop for ServedDream {
    fn drop(&mut self) {
        self.dream.end_now();
        if let Some(owed) = self.owed.take() {
            owed.abort();
        }
    }
}

/// What serves a hosted mob's memory for as long as the mob's identity
/// plane lives, and no longer: the observer of the mob's member events,
/// which holds the mob's handle, the sinks of the mob's memory and its
/// injector, the handle the mob's tracker declares a member's outbound
/// taint through, and the mob's dream (its steward and its store value,
/// and the ask of the dream the mob is owed at its load).
/// The entry that holds the plane ends it before it lets
/// the mob go ([`Self::end`]). What the launch keeps for the life of the
/// process (its two memory slots, the mob's tracker) holds no handle of
/// the mob, and neither its provider nor its store.
pub(crate) struct ServedMobMemory {
    observer: crate::memory::taint::TaintObserverGuard,
    declares_through: Arc<meerkat_mob::MobHandle>,
    dream: Option<ServedDream>,
    distiller: Option<ServedDistiller>,
}

/// The distiller of a hosted mob as what serves the mob's memory holds it:
/// the engine's detached runs (a distillation after a run, at a reset, of a
/// compaction) hold the mob's distiller, and with it the mob's provider,
/// store and transcript source. They end with it: ended
/// ([`ServedMobMemory::end`]) they are stopped and waited for, dropped they
/// are stopped, and none starts afterwards.
struct ServedDistiller(Arc<crate::memory::distiller::DistillerEngine>);

impl Drop for ServedDistiller {
    fn drop(&mut self) {
        self.0.stop_detached_runs();
    }
}

impl ServedMobMemory {
    /// The mob's dream, for the host's dream occurrence: none where the
    /// launch runs no steward.
    pub(crate) fn dream(&self) -> Option<crate::memory_wiring::HostedMobDream> {
        self.dream.as_ref().map(|served| served.dream.clone())
    }

    /// Ask the dream the mob is owed at this load, once: a task of this
    /// value, which ends with it
    /// ([`crate::memory_wiring::HostedMobDream::attempt_when_owed`]).
    pub(crate) fn ask_the_owed_dream(&mut self) {
        if let Some(served) = self.dream.as_mut()
            && served.owed.is_none()
        {
            let dream = served.dream.clone();
            served.owed = Some(tokio::spawn(async move {
                dream.attempt_when_owed().await;
            }));
        }
    }

    /// Test builds only: whether the ask of the dream the mob is owed at
    /// this load ran to its end (a mob with no steward is asked none).
    #[cfg(test)]
    pub(crate) fn owed_dream_is_asked_in_tests(&self) -> bool {
        self.dream.as_ref().is_none_or(|served| {
            served
                .owed
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
        })
    }

    /// End it: a dream of the mob that is under way is stopped and waited
    /// for, and none starts afterwards; the observer is stopped and
    /// joined, so what it held is gone when this returns; the detached
    /// runs of the mob's distiller are stopped and waited for, and none
    /// starts afterwards; and the declarer's handle goes with it. A
    /// declaration that is under way holds the handle until it is answered.
    pub(crate) async fn end(self) {
        let Self {
            observer,
            declares_through,
            dream,
            distiller,
        } = self;
        if let Some(dream) = dream {
            dream.end().await;
        }
        // The observer first: it is what starts a distillation after a
        // member's run. Then the distillations under way.
        observer.abort_and_join().await;
        if let Some(distiller) = distiller {
            distiller.0.end_detached_runs().await;
        }
        drop(declares_through);
    }
}

/// The memory configuration and the store of one mob hosted beside the
/// launch mob, the store proved that mob's own at its incarnation
/// ([`HostMobMemory::store_of_mob`]): what the mob's memory is made over.
struct ProvedMobStore {
    config: crate::identity_first::agent_memory::AgentMemoryConfig,
    store: crate::memory::sqlite_store::SqliteAgentMemoryStore,
}

/// The agent memory of one mob hosted beside the launch mob, as its
/// composition holds it: the mob's provider (its own store behind the
/// write firewall), the configuration its owners are built with, the
/// tracker that firewall reads, the stack's observers of the mob's member
/// events, and the store's steward surface (the provider's own value).
struct MobMemory {
    provider: Arc<dyn crate::identity_first::agent_memory::AgentMemoryProvider>,
    config: crate::identity_first::agent_memory::AgentMemoryConfig,
    taint: crate::memory::taint::SessionTaintTracker,
    sinks: Vec<Arc<dyn crate::MemberAgentEventSink>>,
    steward_store: Arc<dyn crate::memory::capabilities::StewardStore>,
    /// The mob's own distiller, where the launch runs one.
    distiller: Option<Arc<crate::memory::distiller::DistillerEngine>>,
    /// The mob's own dream (its steward over its own store), where the
    /// launch runs a steward.
    dream: Option<crate::memory_wiring::HostedMobDream>,
}

/// The mob context of a hosted mob's own steward: ONE context, the mob the
/// composition was asked for by its typed id, so what that steward promotes
/// becomes that mob's shared memory and nothing else's. Its members are the
/// agents whose record is their row in that mob's identity store, each with
/// its name and its profile and nothing else: a requester's text enters no
/// steward prompt. No purpose is read for it.
struct HostedMobPurpose {
    mob: String,
    /// The mob's own identity runtime. Weak: the runtime holds the steward
    /// that holds this source.
    identities: std::sync::Weak<IdentityRuntime>,
}

impl crate::memory::steward::MobPurposeSource for HostedMobPurpose {
    fn mob_contexts(&self) -> Vec<crate::memory::steward::MobContext> {
        let member_labels = self
            .identities
            .upgrade()
            .map(|identities| identities.row_registered_members())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(identity, profile)| {
                profile.map(|profile| {
                    (
                        identity.as_str().to_string(),
                        std::collections::BTreeMap::from([(
                            "profile".to_string(),
                            profile.as_str().to_string(),
                        )]),
                    )
                })
            })
            .collect();
        vec![crate::memory::steward::MobContext {
            mob: self.mob.clone(),
            purpose: None,
            member_labels,
        }]
    }
}

/// A memory event sink that files nothing: the sink of a mob's memory on a
/// composer that was handed to no runtime.
struct NoMemoryEvents;

impl crate::memory::events::MemoryEventSink for NoMemoryEvents {
    fn emit(&self, _event: crate::memory::events::MemoryTimelineEvent) {}
}

/// The resolver that binds every agent of the mob `mob_id` to that mob's
/// shared memory in the realm of `config`.
pub(crate) fn mob_memory_resolver(
    config: &crate::identity_first::agent_memory::AgentMemoryConfig,
    mob_id: &meerkat_mob::MobId,
) -> Arc<dyn crate::memory::coordinator::MobScopeResolver> {
    Arc::new(crate::memory::coordinator::StaticMobBinding {
        realm: config.realm.clone(),
        mob: mob_id.to_string(),
    })
}

/// The customizer that builds a mob's agents with that mob's agent memory:
/// `base` (the application's and the host's own, which runs first) wrapped
/// with the memory `provider` keeps under `config`. The one constructor of
/// it: the stdio gateway and the runtime builder build their launch mob's
/// here, and the composer of the mobs hosted beside the launch mob builds
/// each of theirs here, with that mob's own provider and configuration.
///
/// `mob` is the mob whose shared memory its agents are listed
/// ([`mob_memory_resolver`]); with `None` they are listed none.
/// `profile_memory_policy` is the host's explicit answer per profile name.
/// `own_memory_only` is the rule of a launch that listed profiles their own
/// memory only, read through the records of the mob these agents are of:
/// each mob's customizer gets a rule over that mob's own records, never
/// another mob's.
pub(crate) fn mob_memory_customizer(
    base: Option<Arc<dyn AgentCustomizer>>,
    provider: Arc<dyn crate::identity_first::agent_memory::AgentMemoryProvider>,
    config: crate::identity_first::agent_memory::AgentMemoryConfig,
    profile_memory_policy: std::collections::BTreeMap<meerkat_mob::ProfileName, bool>,
    mob: Option<&meerkat_mob::MobHandle>,
    own_memory_only: Option<crate::memory::own_memory_only::OwnMemoryOnly>,
) -> crate::identity_first::AgentMemoryCustomizer {
    let resolver = mob.map(|mob| mob_memory_resolver(&config, mob.mob_id()));
    crate::identity_first::AgentMemoryCustomizer::wrap(base, provider, config)
        .with_profile_memory_policy(profile_memory_policy)
        .with_mob_resolver(resolver)
        .with_own_memory_only(own_memory_only)
}

impl HostMobIdentities {
    /// What a launch hands over at the least: its roster provider, asked
    /// with the definition of each mob it answers for, and whether the
    /// launch has a durable runtime store. Everything else is as a launch
    /// that set nothing for its launch mob.
    #[must_use]
    pub fn new(roster: Arc<dyn RosterProvider>, has_runtime_store: bool) -> Self {
        Self {
            roster,
            topology: None,
            customizer: None,
            customizer_of_each_mob: None,
            error_hook: None,
            has_runtime_store,
            bootstrap_mode: IdentityBootstrapMode::default(),
            candidate_launch: false,
            session_repair_scope: None,
            transcript_edit_service: None,
            agent_memory_of_the_launch_mob: false,
            agent_memory: None,
        }
    }

    /// A topology provider for the mobs hosted beside the launch mob: it
    /// would be asked for the edges of each of them. No launch hands one
    /// over: a launch's topology provider, like its roster, is its launch
    /// mob's alone.
    #[must_use]
    pub fn with_topology(mut self, topology: Option<Arc<dyn TopologyProvider>>) -> Self {
        self.topology = topology;
        self
    }

    /// The customizer of those mobs' agents: the launch's one composed
    /// customizer, the application's and the host's own, as it is WITHOUT
    /// the launch mob's agent memory. A launch that keeps agent memory per
    /// mob hands over the customizer it wrapped that memory around, not the
    /// wrapped one: each mob's agents are built with their own mob's memory
    /// ([`Self::with_agent_memory_per_mob`]). One customizer serves every
    /// mob where it reads the mob from each build's own context; a
    /// customizer that holds the runtime services of one identity runtime
    /// is made for each mob ([`Self::with_customizer_of_each_mob`]).
    #[must_use]
    pub fn with_customizer(mut self, customizer: Option<Arc<dyn AgentCustomizer>>) -> Self {
        self.customizer = customizer;
        self
    }

    /// How the customizer of one mob's agents is made from the runtime
    /// services of that mob's identity runtime. It is asked once each time
    /// this composes a mob's identity plane, with that mob's runtime
    /// services, and what it answers is that mob's customizer in place of
    /// [`Self::with_customizer`]'s. It is for a customizer that names the
    /// mob of the services it holds, as a callback bridge does when it
    /// tells `after_create`.
    /// `None` leaves [`Self::with_customizer`]'s for every mob.
    #[must_use]
    pub fn with_customizer_of_each_mob(mut self, of_each_mob: Option<CustomizerOfAMob>) -> Self {
        self.customizer_of_each_mob = of_each_mob;
        self
    }

    /// The launch's error hook.
    #[must_use]
    pub fn with_error_hook(
        mut self,
        error_hook: Option<crate::unified_runtime::ErrorHook>,
    ) -> Self {
        self.error_hook = error_hook;
        self
    }

    /// The launch's identity bootstrap mode.
    #[must_use]
    pub fn with_bootstrap_mode(mut self, bootstrap_mode: IdentityBootstrapMode) -> Self {
        self.bootstrap_mode = bootstrap_mode;
        self
    }

    /// Whether this is a candidate launch, which releases no member's run.
    #[must_use]
    pub fn with_candidate_launch(mut self, candidate_launch: bool) -> Self {
        self.candidate_launch = candidate_launch;
        self
    }

    /// The repair scope of the runtime store this launch opened.
    #[must_use]
    pub fn with_session_repair_scope(mut self, scope: Option<SessionRepairScope>) -> Self {
        self.session_repair_scope = scope;
        self
    }

    /// The launch's transcript edit service.
    #[must_use]
    pub fn with_transcript_edit_service(
        mut self,
        service: Option<Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>>,
    ) -> Self {
        self.transcript_edit_service = service;
        self
    }

    /// The roster this launch handed over is its launch mob's alone (a
    /// roster the launch keeps itself, which answers the same agents
    /// whichever mob it is asked for): it is asked for the launch mob
    /// `launch_mob` only, and every other mob of the host is composed with
    /// no agent of it.
    #[must_use]
    pub fn with_roster_of_the_launch_mob_alone(mut self, launch_mob: meerkat_mob::MobId) -> Self {
        self.roster = Arc::new(RosterOfTheLaunchMobAlone {
            launch_mob,
            roster: self.roster,
        });
        self
    }

    /// Whether the launch keeps agent memory that is its launch mob's alone:
    /// memory that cannot be kept per mob, because a host gave the launch a
    /// memory provider of its own, or because a native host composed the
    /// launch's customizer itself, memory included. Its customizer then
    /// carries that memory, and no agent of another mob may be built with
    /// it: such a launch hands nothing over ([`Self::hand_over_to`]), and
    /// the mobs it hosts beside its launch mob have no identity plane. A
    /// launch that keeps its agent memory in the bundled store hands it
    /// over per mob ([`Self::with_agent_memory_per_mob`]).
    #[must_use]
    pub fn with_agent_memory_of_the_launch_mob(mut self, kept: bool) -> Self {
        self.agent_memory_of_the_launch_mob = kept;
        self
    }

    /// The agent memory the launch keeps in the bundled store: each mob it
    /// hosts beside its launch mob is composed with a memory of its own
    /// ([`HostMobMemory`]). The customizer handed over
    /// ([`Self::with_customizer`]) is then the launch's as it was before the
    /// launch wrapped it with its launch mob's memory. `None`: the launch
    /// keeps no agent memory per mob, and those mobs' agents get none.
    #[must_use]
    pub fn with_agent_memory_per_mob(mut self, memory: Option<HostMobMemory>) -> Self {
        self.agent_memory = memory;
        self
    }

    /// Hand this to the launch's runtime, once, after the launch mob's
    /// identity bootstrap: from then on each mob the host serves beside its
    /// launch mob gets an identity plane of its own when its entry is
    /// built, and the entries built before get theirs now. A launch whose
    /// agent memory is its launch mob's alone hands nothing over
    /// ([`Self::with_agent_memory_of_the_launch_mob`]).
    pub fn hand_over_to(mut self, runtime: &crate::unified_runtime::UnifiedRuntime) {
        if self.agent_memory_of_the_launch_mob {
            tracing::info!(
                "this launch keeps agent memory that is its launch mob's alone: the mobs it \
                 hosts beside its launch mob get no identity plane"
            );
            return;
        }
        // Each mob's memory is entered in the launch's two memory slots,
        // which every member build of the launch's mobs passes. A mob the
        // launch composes beside its launch mob gets a tracker of its own
        // from here on, made when it is first asked for, also by a model
        // call of one of its members before its memory is composed. (A
        // mob's memory events are filed in the console events its entry
        // holds, handed to its composition: nothing is kept here for them.)
        if let Some(memory) = self.agent_memory.as_mut() {
            let of_the_launch_mob = runtime.mob_runtime();
            let slots = MemorySlots {
                taint: of_the_launch_mob.dispatch_taint_slot(),
                side_chat: of_the_launch_mob.side_chat_memory_slot(),
            };
            let content_trust = memory.config.content_trust.clone();
            slots.taint.make_hosted_mob_trackers_with(Arc::new(move || {
                crate::memory::taint::SessionTaintTracker::new(content_trust.clone())
            }));
            memory.slots = Some(slots);
        }
        runtime.compose_host_mob_identities(Arc::new(self));
    }
}

/// A launch's own roster, asked for its launch mob alone
/// ([`HostMobIdentities::with_roster_of_the_launch_mob_alone`]): another
/// mob's definition is answered with no agent.
struct RosterOfTheLaunchMobAlone {
    launch_mob: meerkat_mob::MobId,
    roster: Arc<dyn RosterProvider>,
}

#[async_trait::async_trait]
impl RosterProvider for RosterOfTheLaunchMobAlone {
    async fn roster(
        &self,
        context: &crate::identity_first::RosterContext,
    ) -> Result<Vec<crate::identity_first::DurableAgentSpec>, crate::identity_first::RosterError>
    {
        match &context.mob_definition {
            Some(definition) if definition.id != self.launch_mob => Ok(Vec::new()),
            _ => self.roster.roster(context).await,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl crate::unified_runtime::HostMobIdentityComposer for HostMobIdentities {
    fn builds_agents_only_of_composed_mobs(&self) -> bool {
        self.agent_memory.as_ref().is_some_and(|of_the_launch| {
            of_the_launch.config.recall_failure_policy
                == crate::identity_first::agent_memory::AgentMemoryRecallFailurePolicy::Fail
        })
    }

    async fn compose(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
        console_events: Option<crate::unified_runtime::ConsoleEventStore>,
    ) -> Result<
        crate::unified_runtime::ComposedMobIdentity,
        crate::unified_runtime::MobIdentityNotComposed,
    > {
        // This mob's own agent memory, where the launch keeps agent memory
        // per mob: a store of this mob's own, under this mob's realm. The
        // mob is the one this composition was asked for, by its typed id.
        // Its store is asked FIRST, before anything is built for the mob: a
        // directory under the mob's name that is not proved the mob's
        // refuses the composition, and nothing of the mob is composed over
        // another mob's memory or without a word. The mob's memory engines,
        // the source of the transcripts they read and the mob's dream are
        // made further down, over the store this proved.
        let proved = match self.agent_memory.as_ref() {
            Some(of_the_launch) => of_the_launch
                .store_of_mob(mob)
                .map_err(crate::unified_runtime::MobIdentityNotComposed::MemoryNotTheMobs)?
                .map(|proved| (of_the_launch, proved)),
            None => None,
        };
        // The identity runtime lives inside the composition the mob's owner
        // keeps alive: a back-edge that does not own the mob, as the launch
        // mob's ([`identity_mob_handle`]).
        let handle = mob.handle().observer();
        let bridge = identity_session_bridge(mob, handle.clone());
        let compaction_floors = bridge.compaction_floors();
        // An identity runtime reads and writes neither value: an agent's
        // continuity is in its mob's own store. Each mob's identity runtime
        // is given a pair of its own all the same, so nothing identity-keyed
        // is ever shared by two mobs through them.
        let continuity_store: Arc<dyn ContinuityStore> = Arc::new(
            LocalContinuityStore::in_memory()
                .map_err(|error| IdentityRuntimeError::Internal(error.to_string()))?,
        );
        let runtime_services = AgentRuntimeServices::new(handle.clone());
        // The customizer of this mob's agents before this mob's memory is
        // put around it: one made from this mob's own runtime services
        // where the launch said how, else the launch's one. A maker that
        // refuses composes nothing for the mob, and the composition is
        // asked again.
        let of_this_mob = match self.customizer_of_each_mob.as_ref() {
            Some(of_the_mob) => {
                of_the_mob(&runtime_services).map_err(IdentityRuntimeError::Internal)?
            }
            None => self.customizer.clone(),
        };
        let identity_runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store,
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: format!("gateway-{}-{}", std::process::id(), mob.mob_id()),
                has_runtime_store: self.has_runtime_store,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(bridge)),
                default_timeout: None,
            })
            .with_runtime_services(runtime_services),
        );
        identity_runtime.set_error_hook(self.error_hook.clone());
        if self.candidate_launch {
            identity_runtime.mark_candidate_launch();
        }
        identity_runtime.set_session_repair_scope(self.session_repair_scope.clone());
        // The mob's memory, over the store the first ask of this composition
        // proved the mob's own: its stack and, where the launch runs memory
        // engines, the mob's distiller and steward, the source of the
        // transcripts they read and the mob's dream. None of them is made
        // before that proof, and none for a mob it refused. Its memory
        // events go to the console events the mob's entry holds.
        let memory = proved.and_then(|(of_the_launch, proved)| {
            of_the_launch
                .of_mob(mob, proved, &identity_runtime, console_events)
                .map(|of_the_mob| (of_the_launch, of_the_mob))
        });
        let customizer: Option<Arc<dyn AgentCustomizer>> = match &memory {
            Some((of_the_launch, of_the_mob)) => Some(Arc::new(mob_memory_customizer(
                of_this_mob,
                Arc::clone(&of_the_mob.provider),
                of_the_mob.config.clone(),
                of_the_launch.profile_memory_policy.clone(),
                Some(&handle),
                // Who is listed its own memory only in this mob is read
                // from this mob's own records, through its own identity
                // runtime: the launch mob's records are never asked for an
                // agent of this mob, whatever name it shares.
                crate::memory::own_memory_only::OwnMemoryOnly::listing(
                    of_the_launch.own_memory_only_profiles.clone(),
                    crate::memory::own_memory_only::RecordedLineage::of(&identity_runtime),
                ),
            ))),
            None => of_this_mob,
        };
        // A member materialized outside the roster pass (its first
        // dispatch, a delivery-time repair) runs the installed customizer.
        identity_runtime
            .set_agent_customizer(customizer.clone())
            .await;
        // What this mob's agents are given at a turn, and what a host's
        // memory request for this mob reads and writes.
        let (agent_memory_provider, served_memory) = match memory {
            Some((of_the_launch, of_the_mob)) => {
                let mut injector = crate::identity_first::AgentMemoryRuntimeInjector::new(
                    Arc::clone(&of_the_mob.provider),
                    of_the_mob.config.clone(),
                )
                .with_taint_tracker(of_the_mob.taint.clone())
                .with_mob_resolver(Some(mob_memory_resolver(&of_the_mob.config, mob.mob_id())));
                // The mob's own engines serve the mob's own lifecycle: a
                // rotation of one of its agents is distilled into this
                // mob's memory, and a retirement queues its exit interview
                // in this mob's store.
                if let Some(distiller) = of_the_mob.distiller.clone() {
                    injector = injector.with_distiller(distiller);
                }
                if let Some(dream) = of_the_mob.dream.as_ref() {
                    injector = injector.with_steward(Arc::clone(&dream.steward));
                }
                let served = of_the_launch.serve(
                    mob,
                    &handle,
                    identity_runtime.bridge(),
                    &of_the_mob,
                    &injector,
                );
                identity_runtime.set_agent_memory(Some(injector)).await;
                (Some(of_the_mob.provider), Some(served))
            }
            None => (None, None),
        };
        Ok(crate::unified_runtime::ComposedMobIdentity {
            context: Arc::new(IdentityFirstRuntimeContext::new_with_bootstrap_mode(
                identity_runtime,
                Arc::clone(&self.roster),
                self.topology.clone(),
                customizer,
                // The roster provider is asked with this mob's definition.
                Some(handle.definition().clone()),
                self.bootstrap_mode.clone(),
            )),
            request_services: crate::unified_runtime::IdentityRequestServices {
                agent_memory_provider,
                transcript_edit_service: self.transcript_edit_service.clone(),
                compaction_floors: Some(compaction_floors),
            },
            served_memory,
        })
    }
}

/// The member build wiring a composition installs on its mob and binds to
/// its identity runtime: the customizer tool registry, the spawn customizer
/// that attaches each registered identity's stable customizer tool
/// dispatcher to every build of its member, and Meta's member build
/// preparer, which publishes the member's tools right before Core builds
/// it. `UnifiedRuntimeBuilder`, the stdio gateway and
/// `UnifiedRuntime::bootstrap` all wire their mob here. A runtime the
/// bootstrap composed once had no preparer, and a member it restored was
/// built without its customizer tools.
pub(crate) struct MemberBuildWiring {
    /// The registry the spawn customizer attaches from, when the composition
    /// decided at construction that it composes an agent customizer.
    registry: Option<Arc<CustomizerToolRegistry>>,
    spawn_customizer: Option<Arc<CustomizerToolsSpawnCustomizer>>,
    preparer: Option<Arc<IdentityBuildPreparer>>,
    /// The agent customizer the composition's spec declares
    /// (`MobBootstrapSpec::with_agent_customizer`), which the identity
    /// bootstrap installs.
    declared_customizer: Option<Arc<dyn AgentCustomizer>>,
    /// The slot of the mob this wiring was installed on that binds a
    /// private member's `memory` tool: it reads Core's identity entries
    /// through the identity runtime's bridge once the wiring is bound to one
    /// ([`Self::watch_member_memory_entries`]).
    member_memory: std::sync::OnceLock<crate::memory::side_chat_memory::SideChatMemorySlot>,
    /// Whether the composition knew at construction that an identity
    /// runtime prepares its mob's member builds.
    identity_layer: bool,
}

impl MemberBuildWiring {
    /// The wiring of a composition that knows when it builds its mob whether
    /// it composes an agent customizer (`customizer_tools`) and whether an
    /// identity runtime prepares the mob's member builds (`prepares`).
    pub(crate) fn new(customizer_tools: bool, prepares: bool) -> Self {
        let registry = customizer_tools.then(CustomizerToolRegistry::new);
        Self {
            spawn_customizer: registry
                .clone()
                .map(|registry| Arc::new(CustomizerToolsSpawnCustomizer::new(registry))),
            preparer: prepares.then(|| IdentityBuildPreparer::new(registry.is_some())),
            registry,
            declared_customizer: None,
            member_memory: std::sync::OnceLock::new(),
            identity_layer: prepares,
        }
    }

    /// The wiring of a `UnifiedRuntime::bootstrap` composition whose spec
    /// declares its agent customizer: armed when the mob is built, as
    /// [`Self::new`] arms a composition with a customizer, so a build that
    /// comes before the identity bootstrap is held until the roster pass
    /// publishes its tools. The identity bootstrap installs `customizer` and
    /// binds the preparer ([`Self::bind_at_identity_bootstrap`]).
    pub(crate) fn declared(customizer: Arc<dyn AgentCustomizer>) -> Self {
        Self {
            declared_customizer: Some(customizer),
            ..Self::new(true, true)
        }
    }

    /// The wiring of a `UnifiedRuntime::bootstrap` composition whose spec
    /// declares no agent customizer: it learns only when its host completes
    /// the bootstrap whether it composes one, so its spawn customizer
    /// attaches nothing and its preparer is undecided until
    /// [`Self::bind_at_identity_bootstrap`] finds one, or finds none, or
    /// the activation without an identity context decides that there is
    /// none ([`Self::decide_without_customizer`]). While it is undecided
    /// the preparer refuses, retryable, every build of the member of an
    /// identity whose record is its row: no such member is built before the
    /// host's build step, if it has one, could be asked.
    pub(crate) fn deferred() -> Self {
        Self {
            registry: None,
            spawn_customizer: Some(Arc::new(CustomizerToolsSpawnCustomizer::deferred())),
            preparer: Some(IdentityBuildPreparer::deferred()),
            declared_customizer: None,
            member_memory: std::sync::OnceLock::new(),
            identity_layer: false,
        }
    }

    /// Decide a deferred wiring for a composition that composes no agent
    /// customizer: its preparer stays inert and lets every build through as
    /// Core lowered it (`IdentityBuildPreparer::decide_without_customizer`).
    /// Answers whether this call made the decision, so its caller asks Core
    /// once to seat the identities owed their seat: `false` for a wiring
    /// decided before, either way, and for one that was never deferred.
    pub(crate) fn decide_without_customizer(&self) -> bool {
        self.preparer
            .as_ref()
            .is_some_and(|preparer| preparer.decide_without_customizer())
    }

    /// The preparer this wiring installs on its mob, when it installs one.
    #[cfg(test)]
    pub(crate) fn preparer(&self) -> Option<&Arc<IdentityBuildPreparer>> {
        self.preparer.as_ref()
    }

    /// Install the spawn customizer, composed after those `spec` carries,
    /// and the preparer on `spec`.
    pub(crate) fn install(
        &self,
        spec: crate::mob_handle_runtime::MobBootstrapSpec,
    ) -> crate::mob_handle_runtime::MobBootstrapSpec {
        // The mob's memory slot reads Core's identity entries once this
        // wiring is bound to an identity runtime. A composition that knows
        // at construction that an identity runtime prepares its member
        // builds has an identity layer from the start: its slot waits for
        // that reader before it decides where a private member's records
        // are kept.
        let slot = spec.side_chat_memory_slot();
        if self.identity_layer {
            slot.await_identity_entries();
        }
        let _ = self.member_memory.set(slot);
        let spec = match self.spawn_customizer.clone() {
            Some(customizer) => spec.with_spawn_member_customizer(customizer),
            None => spec,
        };
        match self.preparer.clone() {
            Some(preparer) => spec.with_member_build_preparer(preparer),
            None => spec,
        }
    }

    /// What this wiring gives the builder of a mob the mob set builds for
    /// the host beside its default mob: its spawn customizer and its
    /// preparer, as [`Self::install`] gives them to a spec. The mob's entry
    /// holds no member back.
    pub(crate) fn host_mob_member_wiring(&self) -> crate::mob_handle_runtime::HostMobMemberWiring {
        crate::mob_handle_runtime::HostMobMemberWiring {
            member_build_preparer: self
                .preparer
                .clone()
                .map(|preparer| preparer as Arc<dyn meerkat_mob::MemberBuildPreparer>),
            spawn_member_customizer: self
                .spawn_customizer
                .clone()
                .map(|customizer| customizer as Arc<dyn meerkat_mob::SpawnMemberCustomizer>),
            host_pending_members: Vec::new(),
        }
    }

    /// Withhold every build of this mob's members, for `why`
    /// (`IdentityBuildPreparer::withhold_builds`). Answers whether the
    /// wiring has a preparer to withhold them: one without it builds its
    /// members as Core lowers them.
    pub(crate) fn withhold_member_builds(
        &self,
        why: crate::memory::sqlite_store::HostedAgentsNotBuilt,
    ) -> bool {
        self.preparer
            .as_ref()
            .map(|preparer| preparer.withhold_builds(why))
            .is_some()
    }

    /// [`Self::withhold_member_builds`] where nothing is withheld yet.
    pub(crate) fn withhold_member_builds_unless_withheld(
        &self,
        why: crate::memory::sqlite_store::HostedAgentsNotBuilt,
    ) {
        if let Some(preparer) = self.preparer.as_ref() {
            preparer.withhold_builds_unless_withheld(why);
        }
    }

    /// End [`Self::withhold_member_builds`]: the members whose build was
    /// refused meanwhile (`IdentityBuildPreparer::release_withheld_builds`).
    pub(crate) fn release_withheld_member_builds(&self) -> Vec<meerkat_mob::ids::AgentIdentity> {
        self.preparer
            .as_ref()
            .map(|preparer| preparer.release_withheld_builds())
            .unwrap_or_default()
    }

    /// Whether the composition's spec declared its agent customizer: the
    /// wiring then holds the identity members the mob builds until an
    /// identity bootstrap binds it.
    pub(crate) fn is_declared(&self) -> bool {
        self.declared_customizer.is_some()
    }

    /// The registry the composition installs on its identity runtime, the
    /// one its spawn customizer attaches from.
    pub(crate) fn customizer_tool_registry(&self) -> Option<Arc<CustomizerToolRegistry>> {
        self.registry.clone()
    }

    /// Bind the preparer to `runtime` once its customizer, customizer tool
    /// registry and identity records are installed: the builds it held
    /// before are pending on the runtime, and the roster pass publishes them.
    pub(crate) async fn bind(
        &self,
        runtime: &Arc<IdentityRuntime>,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    ) {
        self.watch_member_memory_entries(runtime);
        self.watch_identity_entries(runtime);
        if let Some(preparer) = self.preparer.as_ref() {
            preparer.bind(runtime, session_service).await;
        }
    }

    /// From now on the mob's memory slot reads Core's identity entries
    /// through `runtime`'s session bridge
    /// ([`crate::memory::side_chat_memory::SideChatMemorySlot::watch_identity_entries`]).
    fn watch_member_memory_entries(&self, runtime: &IdentityRuntime) {
        if let (Some(slot), Some(bridge)) = (self.member_memory.get(), runtime.bridge()) {
            slot.watch_identity_entries(bridge);
        }
    }

    /// From now on the spawn customizer attaches an identity's dispatcher to
    /// each build of a member Core holds as an identity entry, read through
    /// `runtime`'s session bridge
    /// ([`CustomizerToolsSpawnCustomizer::watch_identity_entries`]).
    fn watch_identity_entries(&self, runtime: &IdentityRuntime) {
        if let (Some(customizer), Some(bridge)) = (self.spawn_customizer.as_ref(), runtime.bridge())
        {
            customizer.watch_identity_entries(bridge);
        }
    }

    /// Bind a `UnifiedRuntime::bootstrap` wiring to the identity runtime of
    /// `context`, before its identity bootstrap lifts the mob, composing the
    /// runtime as `UnifiedRuntimeBuilder` composes it.
    ///
    /// The customizer is the one the spec declared; without a declaration,
    /// the one installed on the runtime, else the one the context carries.
    /// A declared customizer allows the runtime and the context to carry the
    /// same customizer or none: any other one is refused, as is a runtime
    /// whose customizer tool registry is not the one the mob was built with.
    /// Without any customizer a deferred wiring is decided for none: its
    /// preparer stays inert, and the call that made that decision asks Core
    /// once to seat the identities whose record is their row that are owed
    /// their seat (the preparer refused their fresh seat while it was
    /// undecided). A later identity bootstrap that finds a customizer still
    /// binds it.
    /// With one, it becomes the runtime's own when none is installed, the
    /// runtime gets the composition's registry when it has none, the spawn
    /// customizer attaches from the runtime's registry, and the preparer
    /// publishes tools and is bound. A refusal installs none of them.
    pub(crate) async fn bind_at_identity_bootstrap(
        &self,
        context: &IdentityFirstRuntimeContext,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    ) -> Result<(), String> {
        let runtime = &context.runtime;
        self.watch_member_memory_entries(runtime);
        let installed = runtime.installed_agent_customizer().await;
        let customizer = match self.declared_customizer.as_ref() {
            Some(declared) => {
                let other = |carried: &Arc<dyn AgentCustomizer>| !Arc::ptr_eq(carried, declared);
                if context.customizer.as_ref().is_some_and(other) {
                    return Err(
                        "conflicting agent customizers: the mob's spec declares one \
                         (MobBootstrapSpec::with_agent_customizer) and the identity context \
                         carries another; the context carries the declared customizer or none"
                            .to_string(),
                    );
                }
                if installed.as_ref().is_some_and(other) {
                    return Err(
                        "conflicting agent customizers: the mob's spec declares one \
                         (MobBootstrapSpec::with_agent_customizer) and the identity runtime has \
                         another installed; the runtime installs the declared customizer or none"
                            .to_string(),
                    );
                }
                Arc::clone(declared)
            }
            None => match installed.clone().or_else(|| context.customizer.clone()) {
                Some(customizer) => customizer,
                None => {
                    // The composition composes no agent customizer: the
                    // preparer lets every build through from now on. The
                    // fresh seats it refused while that was undecided are
                    // owed, and the call that makes the decision asks Core
                    // once to seat them. A mob this bootstrap has not
                    // lifted yet seats nothing here: its lift runs the
                    // same pass.
                    if self.decide_without_customizer() {
                        runtime.seat_created_identities_owed_their_seat().await;
                    }
                    return Ok(());
                }
            },
        };
        let registry = match (
            runtime.installed_customizer_tool_registry().await,
            self.registry.as_ref(),
        ) {
            (Some(runtime_registry), Some(composed))
                if !Arc::ptr_eq(&runtime_registry, composed) =>
            {
                return Err(
                    "conflicting customizer tool registries: the mob was built with its own \
                     and the identity runtime has another installed; the runtime installs none"
                        .to_string(),
                );
            }
            (Some(runtime_registry), _) => runtime_registry,
            (None, composed) => {
                let registry = composed
                    .cloned()
                    .unwrap_or_else(CustomizerToolRegistry::new);
                runtime
                    .set_customizer_tool_registry(Some(Arc::clone(&registry)))
                    .await;
                registry
            }
        };
        if installed.is_none() {
            runtime.set_agent_customizer(Some(customizer)).await;
        }
        if let Some(customizer) = self.spawn_customizer.as_ref() {
            customizer.arm(registry);
        }
        self.watch_identity_entries(runtime);
        if let Some(preparer) = self.preparer.as_ref() {
            preparer.bind_publishing(runtime, session_service).await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod member_keyed_continuity_tests {
    use super::*;
    use crate::identity_first::{DurabilityPolicy, IdentityRuntimeConfig};

    /// The continuity file keys its records by identity alone
    /// (`continuity_records(identity PRIMARY KEY)`), which two mobs with a
    /// member of the same name would share. It is opened here for its
    /// fencing high-water and handed to the identity runtime, which keeps
    /// no handle of it: nothing a running host does can write a record
    /// there, so the file holds none and needs no mob in its key. Core's
    /// mob store, keyed by mob and identity, is where a member's binding
    /// lives.
    #[tokio::test]
    async fn member_keyed_continuity_file_is_opened_and_never_written_by_a_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("continuity.sqlite3");
        let substrate = open_identity_substrate(&path).await.expect("substrate");
        let store = Arc::clone(&substrate.continuity_store);
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: substrate.continuity_store,
            lease_provider: substrate.lease_provider,
            runtime_instance_id: "member-keyed-continuity".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        assert_eq!(
            Arc::strong_count(&store),
            1,
            "the identity runtime keeps no handle of the continuity store it is given"
        );
        drop(runtime);
        drop(store);
        let conn = rusqlite::Connection::open(&path).expect("the opened file");
        for table in ["continuity_records", "session_snapshots"] {
            let rows: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("count");
            assert_eq!(rows, 0, "{table} holds no identity-keyed row");
        }
    }
}
