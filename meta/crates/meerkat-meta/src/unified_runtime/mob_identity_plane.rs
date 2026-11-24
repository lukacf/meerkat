//! The identity plane of a mob a host serves beside its default mob.
//!
//! The default mob's identity context is composed by its launch, once, at
//! the launch. A mob the launch's mob set builds for the host beside it is
//! built at its create, at every load of its store and at a restart, so
//! what such a mob needs composed hangs on its entry's build: the launch
//! says once how it composes the identity context of a mob
//! ([`HostMobIdentityComposer`]), and each entry asks for its own
//! ([`MobEntry::compose_identity_plane`]).
//!
//! Whether a mob has its identity plane is the identity context in its
//! entry's cell and nothing else: an entry whose cell is empty is asked
//! again, by the task that follows the mob set and by each request that
//! resolves the mob. Nothing is composed for a mob that is not loaded, and
//! a host of one mob composes nothing.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_mob::{MobError, MobHandle};

use super::IdentityRequestServices;
use super::mob_entries::HostOfEntries;
use super::mob_entry::MobEntry;
use crate::identity_creation::{
    HeldChangeFamilies, HeldFamily, LaunchIdentityPolicy, attach_host_mob_family,
};
use crate::identity_first::{IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeError};
use crate::mob_handle_runtime::MobRuntime;

/// What a launch composed for one mob it hosts beside its default mob.
pub(crate) struct ComposedMobIdentity {
    /// The mob's identity context: an identity runtime over that mob's own
    /// handle, with the launch's roster provider, topology provider and
    /// customizer, and that mob's definition.
    pub(crate) context: Arc<IdentityFirstRuntimeContext>,
    /// What a request to that mob's identity plane needs beside the
    /// context.
    pub(crate) request_services: IdentityRequestServices,
    /// What serves that mob's agent memory while its identity plane
    /// lives, when the launch keeps agent memory for the mob: it holds
    /// the mob, so the entry keeps it and ends it with the plane.
    pub(crate) served_memory: Option<crate::gateway_wiring::ServedMobMemory>,
}

/// Why a launch composed no identity plane for a mob it hosts beside its
/// default mob.
#[derive(Debug)]
pub(crate) enum MobIdentityNotComposed {
    /// The identity runtime, or what it is built over, failed.
    Identity(IdentityRuntimeError),
    /// The directory under the mob's name in the launch's agent memory is
    /// not proved to be that mob's: the mob is given no identity plane, so
    /// the composition seats and starts none of its agents and the mob is
    /// served no agent memory, neither another mob's nor the launch's.
    /// Nothing in the directory was changed. An agent the mob's own records
    /// hold can still be made live by a wake through the mob's handle, as
    /// for any mob without an identity plane: it is given no agent memory.
    /// That is the launch's default (the recall failure policy `skip`).
    /// Under the policy `fail` no agent of the mob is built while the
    /// directory is refused: the mob's entry withholds its member builds,
    /// and a wake of a stored agent is refused, to be retried.
    MemoryNotTheMobs(crate::memory::sqlite_store::HostedMemoryRefusal),
}

/// What the host's log says of a mob whose memory directory is refused
/// where the launch's recall failure policy is `skip`, the default.
pub(super) const REFUSED_AND_ITS_AGENTS_RUN_WITHOUT_MEMORY: &str = "the mob is given no identity plane: it is served no agent memory and its \
     composition starts none of its agents; nothing in the directory was changed. An agent \
     the mob's own records hold can still be woken, and then runs without agent memory. \
     This is logged once while the mob stays loaded, and again when the reason changes";

/// What the host's log says of such a mob where the launch's recall failure
/// policy is `fail`.
pub(super) const REFUSED_AND_NONE_OF_ITS_AGENTS_IS_BUILT: &str = "the mob is given no identity plane: it is served no agent memory, and by the \
     launch's recall failure policy `fail` none of its agents is built until the directory \
     is the mob's: a wake of one is refused, to be retried; nothing in the directory was \
     changed. This is logged once while the mob stays loaded, and again when the reason \
     changes";

impl From<IdentityRuntimeError> for MobIdentityNotComposed {
    fn from(error: IdentityRuntimeError) -> Self {
        Self::Identity(error)
    }
}

impl std::fmt::Display for MobIdentityNotComposed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(error) => write!(f, "{error}"),
            Self::MemoryNotTheMobs(refusal) => write!(f, "{refusal}"),
        }
    }
}

/// How a launch composes the identity context of a mob it hosts beside its
/// default mob. A launch that has an identity plane gives its runtime one
/// composer (`UnifiedRuntime::compose_host_mob_identities`); the runtime
/// asks it for each such mob when that mob's entry is built, with the mob's
/// own wrapper (its handle, its definition, its session service). It is
/// never asked for the default mob, for a mob a member created, or for a
/// mob that is not loaded.
///
/// The composer does not say where the identity runtime keeps its records:
/// the entry binds them to the host's metadata store under its own mob's
/// id, whatever the composer told the identity runtime.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait HostMobIdentityComposer: Send + Sync {
    /// Compose the identity context of `mob`. An error composes nothing:
    /// the mob stays served without an identity plane, and the composition
    /// is asked again.
    ///
    /// `console_events` is the console event store the mob's entry holds,
    /// where it holds one: what the composition files for the mob (its
    /// memory events) is filed there. A composer asks the host for no
    /// store by the mob's id: an entry that ended keeps its own store, and
    /// the id may name another mob's by then. `None`: nothing is filed.
    async fn compose(
        &self,
        mob: &MobRuntime,
        console_events: Option<super::ConsoleEventStore>,
    ) -> Result<ComposedMobIdentity, MobIdentityNotComposed>;

    /// Whether this launch builds an agent of a mob it hosts only once the
    /// mob is composed: a launch whose recall failure policy is `fail`
    /// gives no agent a build without the mob's agent memory. The member
    /// builds of such a launch's mobs are withheld from the mob's build
    /// until its first composition succeeds
    /// (`HostedAgentsNotBuilt::MemoryNotComposedYet`), and while the
    /// directory under the mob's name is refused
    /// (`HostedAgentsNotBuilt::MemoryRefused`). Every other launch builds
    /// a mob's members as the mob's own runtime lowers them.
    ///
    /// A composition that fails is asked again by the next request that
    /// names the mob or by the table's next pass, on no timer: until one
    /// succeeds the mob's agents stay unbuilt and their wakes are refused,
    /// to be retried. A reason that stands is replaced only by a refusal
    /// of the mob's memory: after such a refusal, a later failure of
    /// another kind leaves the refusal standing as the reason.
    fn builds_agents_only_of_composed_mobs(&self) -> bool {
        false
    }
}

/// Where the identity plane of a resolved mob stands, as a door reads it
/// without waiting ([`super::MobResolver::identity_plane_standing`]). It is
/// a reading, kept nowhere: the identity context in the mob's entry and the
/// launch's composer are the facts.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "answered to a door that must not act on a mob whose identity plane is owed; none reads it yet"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentityPlaneStanding {
    /// The mob has its identity context.
    Attached,
    /// The launch composes the identity planes of its other mobs and this
    /// mob's is not attached yet: its composition has not run, is under
    /// way, or failed and is tried again at the next resolve of the mob.
    /// Asking again later is right.
    Owed,
    /// Nothing composes one for this mob: a launch that composes none, or
    /// the default mob of a launch without an identity plane.
    NotOwed,
}

/// How long the end of an entry waits for its mob's lifecycle projection to
/// reach its journal's head once more. The mob's actor closed before an
/// entry ends, so the projection has nothing left to read.
const ENTRY_END_PROJECTION_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// Have the flows of the mob `handle` serves pass `runtime`'s identity
/// barrier: before a flow run of that mob is minted, every identity
/// `runtime` registered is made a member of the mob.
pub(super) fn install_flow_target_provisioner(handle: &MobHandle, runtime: &Arc<IdentityRuntime>) {
    let identity_runtime = Arc::downgrade(runtime);
    handle.install_flow_target_provisioner(Arc::new(move || {
        let identity_runtime = identity_runtime.clone();
        Box::pin(async move {
            let runtime = identity_runtime.upgrade().ok_or_else(|| {
                MobError::Internal(
                    "identity-first flow provisioner is no longer available".to_string(),
                )
            })?;
            // The barrier makes every registered identity a member of
            // Core's mob before the flow run id is minted, and
            // materializes none: each step's delivery reaches a
            // Dormant member through Core's own wake. Only an
            // identity Core does not hold yet is seated, in one step.
            tracing::info!(
                cause = "flow_target_provisioning",
                "identity-first flow barrier is seating every registered identity Core \
                 does not hold yet; nothing is materialized"
            );
            let started = std::time::Instant::now();
            let outcome = runtime
                .materialize_all_required_for_entered_flow()
                .await
                .map(|records| records.len())
                .map_err(|error| {
                    MobError::Internal(format!("identity-first flow membership failed: {error}"))
                });
            match &outcome {
                Ok(members) => tracing::info!(
                    members = *members,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    cause = "flow_target_provisioning",
                    "identity-first flow barrier complete"
                ),
                Err(error) => tracing::warn!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    cause = "flow_target_provisioning",
                    %error,
                    "identity-first flow barrier failed"
                ),
            }
            outcome.map(|_| ())
        })
    }));
}

impl super::UnifiedRuntime {
    /// Say how this launch composes the identity context of a mob it hosts
    /// beside its default mob. A launch that has an identity plane calls it
    /// once, after its default mob's identity bootstrap
    /// ([`crate::gateway_wiring::HostMobIdentities::hand_over_to`]): from
    /// then on each such mob gets an identity plane of its own when its
    /// entry is built (at its create, at each load of its store, at a
    /// restart), and the entries built before get theirs now. The first
    /// composer stands. A launch that never calls it gives those mobs no
    /// identity plane.
    pub(crate) fn compose_host_mob_identities(&self, composer: Arc<dyn HostMobIdentityComposer>) {
        self.other_mobs().compose_identities(composer);
    }

    /// The dreams of the mobs this host serves beside its default mob
    /// whose identity planes live at the moment the answer is asked: what
    /// the host's dream occurrence dreams for after its default mob. It
    /// reads the entries the runtime holds, loads no mob and keeps nothing.
    pub(crate) fn hosted_mob_dreams(&self) -> crate::memory_wiring::HostedMobDreams {
        let entries = Arc::downgrade(self.other_mobs());
        Arc::new(move || {
            entries
                .upgrade()
                .map(|entries| {
                    entries
                        .held()
                        .into_iter()
                        .filter(|entry| !entry.actor_closed())
                        .filter_map(|entry| entry.dream_of_its_mob())
                        .collect()
                })
                .unwrap_or_default()
        })
    }
    /// Say this launch's identity policy to the mobs it hosts beside its
    /// launch mob, once: each such mob's identity plane is composed with
    /// the family of that mob's own agents under it, and the mobs composed
    /// before are given it now. The first policy stands. A launch that
    /// never says one leaves those mobs their reads alone.
    pub(crate) fn say_identity_policy_to_host_mobs(&self, policy: LaunchIdentityPolicy) {
        self.other_mobs().say_identity_policy(policy);
    }

    /// The holders of the held changes of every mob this host serves, as
    /// the launch's one driver of the `identity.*` decision kinds asks
    /// them: the launch mob `launch_mob`'s through its identity runtime
    /// `launch`, and every other mob's through the table of the host's
    /// other mobs. Both are held weakly: the answer keeps no mob and no
    /// runtime alive.
    pub(crate) fn held_change_families(
        &self,
        launch_mob: meerkat_mob::MobId,
        launch: &Arc<IdentityRuntime>,
    ) -> Arc<dyn HeldChangeFamilies> {
        Arc::new(HostHeldChangeFamilies {
            launch_mob,
            launch: Arc::downgrade(launch),
            others: Arc::downgrade(self.other_mobs()),
        })
    }

    /// The identity runtime of the mob `mob_id`, a mob this host serves
    /// beside its launch mob, when the host runs that mob now and its
    /// identity plane is composed. It loads nothing: `None` for a mob that
    /// is not loaded, and for an id no such mob has.
    pub(crate) fn identity_runtime_of_host_mob(
        &self,
        mob_id: &str,
    ) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.other_mobs()
            .identity_runtime_of(&meerkat_mob::MobId::from(mob_id))
    }
}

/// The holders of the held changes of the mobs of one host
/// ([`crate::UnifiedRuntime::held_change_families`]). Which holder answers
/// for a held change is decided by the mob id the host's own family wrote
/// into the held body, against the host's own records: the launch mob's
/// id, or the table's entry of a mob the host runs now.
struct HostHeldChangeFamilies {
    launch_mob: meerkat_mob::MobId,
    launch: std::sync::Weak<IdentityRuntime>,
    others: std::sync::Weak<super::mob_entries::MobEntries>,
}

#[async_trait]
impl HeldChangeFamilies for HostHeldChangeFamilies {
    async fn of(&self, mob_id: &str) -> HeldFamily {
        if mob_id == self.launch_mob.as_str() {
            return match self
                .launch
                .upgrade()
                .and_then(|identities| identities.held_changes())
            {
                Some(holder) => HeldFamily::Here(Box::new(holder)),
                None => HeldFamily::Away,
            };
        }
        match self.others.upgrade() {
            Some(others) => {
                others
                    .held_changes_of(&meerkat_mob::MobId::from(mob_id))
                    .await
            }
            // The runtime is gone: nothing is carried out, and nothing is
            // decided about the mob.
            None => HeldFamily::Away,
        }
    }
}

impl MobEntry {
    /// Attach the family of this mob's long-term agents to its identity
    /// runtime, when it has one: its reads, and its changes under `launch`,
    /// the launch's identity policy. A composition of the mob's identity
    /// plane under way ends first; an entry whose actor closed, and one
    /// without an identity plane, are left as they are.
    pub(super) async fn attach_identity_family(&self, launch: Option<&LaunchIdentityPolicy>) {
        let _one_at_a_time = self.identity_composition.lock().await;
        if self.actor_closed() {
            return;
        }
        if let Some(context) = self.identity_first_context.get() {
            attach_host_mob_family(&self.mob_runtime, &context.runtime, launch);
        }
    }
}

impl MobEntry {
    /// The dream of this entry's mob, while what serves the mob's agent
    /// memory lives: none for a mob without a steward of its own, and none
    /// once the entry ended its identity plane.
    fn dream_of_its_mob(&self) -> Option<crate::memory_wiring::HostedMobDream> {
        self.served_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(crate::gateway_wiring::ServedMobMemory::dream)
    }

    /// Have what serves this mob's memory ask the dream the mob is owed at
    /// this load. The ask is a task of what serves the mob's memory, and
    /// ends with it ([`Self::end_identity_plane`]).
    fn ask_the_owed_dream(&self) {
        if let Some(served) = self
            .served_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            served.ask_the_owed_dream();
        }
    }

    /// Test builds only: whether this mob's memory is served and the ask
    /// of the dream the mob is owed at this load ran to its end.
    #[cfg(test)]
    pub(super) fn owed_dream_is_asked_in_tests(&self) -> bool {
        self.served_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(crate::gateway_wiring::ServedMobMemory::owed_dream_is_asked_in_tests)
    }

    /// Whether this mob's identity plane is composed and no composition of
    /// it is under way: read without waiting. A composition under way
    /// holds the entry's composition lock from before the context is
    /// attached until the roster pass ended.
    pub(super) fn identity_plane_is_composed(&self) -> bool {
        self.identity_first_context.get().is_some() && self.identity_composition.try_lock().is_ok()
    }

    /// Test builds only: the refusals the compositions of this mob answered
    /// for its memory directory, in order.
    #[cfg(test)]
    pub(super) fn memory_refusals_in_tests(
        &self,
    ) -> Vec<crate::memory::sqlite_store::HostedMemoryRefusal> {
        self.memory_refusals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Test builds only: the refusals the host's log carried for this
    /// mob's memory directory, in order: one for each line written.
    #[cfg(test)]
    pub(super) fn memory_refusal_lines_in_tests(
        &self,
    ) -> Vec<crate::memory::sqlite_store::HostedMemoryRefusal> {
        self.memory_refusal_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(refusal, _)| refusal.clone())
            .collect()
    }

    /// Test builds only: the words of each line the host's log carried for
    /// this mob's memory directory, in order.
    #[cfg(test)]
    pub(super) fn memory_refusal_line_words_in_tests(&self) -> Vec<&'static str> {
        self.memory_refusal_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(_, words)| *words)
            .collect()
    }

    /// Test builds only: why this mob's member builds are withheld, while
    /// they are.
    #[cfg(test)]
    pub(super) fn agents_not_built_in_tests(
        &self,
    ) -> Option<crate::memory::sqlite_store::HostedAgentsNotBuilt> {
        self.bootstrap_member_build_wiring
            .as_ref()
            .and_then(|wiring| wiring.preparer())
            .and_then(|preparer| preparer.withheld_in_tests())
    }

    /// Close this mob's identity runtime to requests and end the work it
    /// owns for them, in the order of the default mob's at the runtime's
    /// shutdown: its foreground admission closes, its bootstrap is
    /// cancelled and joined, and the operations it admitted before are
    /// joined. A mob without an identity plane has nothing to close.
    pub(super) async fn close_identity_requests(&self) {
        let Some(context) = self.identity_first_context.get() else {
            return;
        };
        context.runtime.close_foreground_operations();
        context.runtime.cancel_identity_bootstrap().await;
        context.runtime.join_foreground_operations().await;
    }

    /// Stop the lifecycle projection of this mob's identity runtime, by
    /// `deadline` at the latest: what it did not reach runs at the mob's
    /// next composition, from the record the store holds.
    pub(super) async fn stop_identity_lifecycle_projection(&self, deadline: tokio::time::Instant) {
        if let Some(context) = self.identity_first_context.get() {
            context.runtime.stop_lifecycle_projection(deadline).await;
        }
    }

    /// The end of this mob's identity runtime with its entry (the mob set
    /// unloaded or destroyed the mob, or the runtime shut down):
    /// [`Self::close_identity_requests`], then what served the mob's agent
    /// memory ends, then the lifecycle projection stops within
    /// [`ENTRY_END_PROJECTION_WINDOW`]. The first and the last are done
    /// once: a runtime that shut down did them before, and they do nothing
    /// again.
    pub(super) async fn end_identity_plane(&self) {
        self.close_identity_requests().await;
        // What served the mob's agent memory holds the mob (its handle,
        // its memory). It ends after the requests it serves: no operation
        // of this mob's identity runtime is admitted or runs any more, so
        // none runs with nothing observing it. It is ended and joined, so
        // nothing of it is left when the entry lets the mob go.
        let served_memory = self
            .served_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(served_memory) = served_memory {
            served_memory.end().await;
        }
        self.stop_identity_lifecycle_projection(
            tokio::time::Instant::now() + ENTRY_END_PROJECTION_WINDOW,
        )
        .await;
    }

    /// Give this entry's mob its identity plane, when it has none: ask
    /// `composer`, the launch's, for the mob's identity context, attach it,
    /// and seat the agents the launch's roster provider answers for this
    /// mob's definition. One composition runs at a time for an entry, and a
    /// caller that comes while one runs waits for it: a request that
    /// resolves the mob never meets it half composed.
    ///
    /// In the order of the default mob's identity install and bootstrap:
    /// the identity records are bound to the host's metadata store under
    /// this mob's id, the context is attached
    /// ([`Self::attach_identity_context`]), the family of this mob's own
    /// long-term agents is attached to its identity runtime (its reads,
    /// and its changes under the launch's identity policy `launch` once
    /// the launch said one), the
    /// mob's flows are given this identity runtime's barrier, the roster is
    /// asked and applied, and the lifecycle side effects of that pass are
    /// settled. The composition holds the entry's composition lock
    /// throughout, so a request that resolves the mob never meets its
    /// identity plane without its family.
    ///
    /// A composition or an attach that fails leaves the cell empty and is
    /// logged: the next caller asks again. A roster that cannot be asked or
    /// applied leaves the context attached; the identity runtime's own
    /// bootstrap record keeps that failure, and a refresh of this mob's
    /// desired topology asks again.
    pub(super) async fn compose_identity_plane(
        &self,
        host: &HostOfEntries,
        composer: &dyn HostMobIdentityComposer,
        launch: &std::sync::OnceLock<LaunchIdentityPolicy>,
    ) {
        let _one_at_a_time = self.identity_composition.lock().await;
        if self.identity_first_context.get().is_some() || self.actor_closed() {
            return;
        }
        let mob_id = self.mob_id().clone();
        // The entry's own console events, the store it holds from its
        // making: the composer is handed that store and asks for none by
        // the id.
        let console_events = self.console_log.kept();
        let ComposedMobIdentity {
            context,
            request_services,
            served_memory,
        } = match composer.compose(&self.mob_runtime, console_events).await {
            Ok(composed) => composed,
            Err(MobIdentityNotComposed::MemoryNotTheMobs(refusal)) => {
                #[cfg(test)]
                self.memory_refusals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(refusal.clone());
                // A launch that builds an agent only of a composed mob (its
                // recall failure policy is `fail`) builds none of this
                // mob's agents while its memory directory is refused: the
                // mob's member builds are withheld for the refusal, so a
                // wake of a stored agent is refused, to be retried. Said
                // again at each refused ask, which changes nothing.
                let none_is_built = composer.builds_agents_only_of_composed_mobs()
                    && self
                        .bootstrap_member_build_wiring
                        .as_ref()
                        .is_some_and(|wiring| {
                            wiring.withhold_member_builds(
                                crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryRefused(
                                    refusal.clone(),
                                ),
                            )
                        });
                let words = if none_is_built {
                    REFUSED_AND_NONE_OF_ITS_AGENTS_IS_BUILT
                } else {
                    REFUSED_AND_ITS_AGENTS_RUN_WITHOUT_MEMORY
                };
                // The host's log carries a refusal of this mob once while
                // the mob stays loaded (what was logged is this entry's),
                // and again when it changes: a refused mob is asked at every
                // request that names it, and each ask is refused whether
                // or not it is logged. The refusal's own words stay with
                // the host: they name a path of the memory root.
                let mut logged = self
                    .memory_refusal_logged
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if logged.as_ref() != Some(&refusal) {
                    tracing::error!(%mob_id, %refusal, "{words}");
                    #[cfg(test)]
                    self.memory_refusal_lines
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((refusal.clone(), words));
                    *logged = Some(refusal);
                }
                return;
            }
            Err(MobIdentityNotComposed::Identity(error)) => {
                // A launch that builds an agent only of a composed mob
                // builds none of this mob's agents until a composition
                // succeeds: said here for a mob that was built before the
                // launch said how it composes, whose builds were not
                // withheld from its build.
                if composer.builds_agents_only_of_composed_mobs()
                    && let Some(wiring) = self.bootstrap_member_build_wiring.as_ref()
                {
                    wiring.withhold_member_builds_unless_withheld(
                        crate::memory::sqlite_store::HostedAgentsNotBuilt::MemoryNotComposedYet,
                    );
                }
                tracing::warn!(
                    %mob_id,
                    %error,
                    "the identity plane of a mob of the host could not be composed; the mob is \
                     served without one until a later ask composes it"
                );
                return;
            }
        };
        // The records of this mob's identity runtime are keyed by this
        // mob's id, in the host's metadata store: the entry says so, not
        // the composer.
        context
            .runtime
            .set_identity_records(Arc::clone(&host.persistent_metadata), mob_id.to_string());
        if let Err(error) = self.attach_identity_context(Arc::clone(&context)).await {
            tracing::warn!(
                %mob_id,
                %error,
                "the identity context composed for a mob of the host could not be attached; \
                 the mob is served without an identity plane until a later ask composes it"
            );
            // What the composition started for the mob's memory holds the
            // mob: it ends here, with the plane that was not attached.
            if let Some(served_memory) = served_memory {
                served_memory.end().await;
            }
            return;
        }
        // The family of this mob's own long-term agents: every door that
        // resolves the mob reads and changes its agents through it. The
        // launch's policy is read here, under the composition lock: one
        // said meanwhile is not missed.
        attach_host_mob_family(&self.mob_runtime, &context.runtime, launch.get());
        *self
            .served_memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = served_memory;
        *self
            .identity_request_services
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request_services);
        // A flow of this mob passes this mob's identity barrier: every
        // identity its identity runtime registered is a member of the mob
        // before a flow run is minted.
        install_flow_target_provisioner(&self.mob_runtime.handle(), &context.runtime);
        match context.refresh_desired_topology_tracked().await {
            // The side effects of the pass's own lifecycle steps run before
            // the composition answers, as at the default mob's bootstrap.
            Ok(_) => {
                context.runtime.settle_lifecycle_projection().await;
                // The mob's agents are seated: the dream the mob missed
                // while it was not loaded is asked now. A launch that does
                // not speak for the composition asks none, as it runs no
                // dream occurrence.
                if self.mob_runtime.speaks_for_composition() {
                    self.ask_the_owed_dream();
                }
            }
            Err(error) => tracing::warn!(
                %mob_id,
                %error,
                "the roster of a mob of the host could not be asked or applied; its identity \
                 plane is attached with no agent seated by this pass, and a refresh of the \
                 mob's desired topology asks again"
            ),
        }
    }
}
