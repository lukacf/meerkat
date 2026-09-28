//! Off-loop local spawn preparation (#1249).
//!
//! A local `Spawn` used to run its whole preparation inline on the actor
//! loop: session reads that can decode a whole WholeBlob document or wait
//! behind a busy session task (`is_member_active`), resume-authority
//! checks, fork-source history reads, and agent-config builds that read
//! skill files. During a cold boot with ~160 identities those steps held the
//! serialized loop for up to 170 s, starving every peer send, admission, and
//! lifecycle command behind them.
//!
//! The actor now keeps only O(1) machine work inline (command admission,
//! profile resolution and machine-owned authorization) and hands the heavy
//! part to a supervised, concurrency-bounded task. The actor retains custody
//! of the caller's reply, the respawn origin, and the identity actuation
//! permit in a keyed table; the task owns no custody and reports one typed
//! completion, [`MobCommand::SpawnPreparationSettled`], which re-enters the
//! loop, re-checks admission against current state, and continues the
//! ordinary spawn ladder. A lifecycle transition that fails pending spawns
//! also cancels every in-flight preparation and settles its custody.

use super::*;

/// Upper bound on concurrently running spawn preparations (and deferred
/// resume request preparations inside provisioning tasks).
///
/// Preparations are pure reads and config builds that each may decode one
/// durable session document. The bound keeps a cold-boot burst of spawns
/// from running unbounded concurrent durable reads; a waiting preparation
/// awaits a permit off the actor, so the loop itself never waits on it.
pub(super) const SPAWN_PREPARATION_CONCURRENCY: usize = 8;

/// Everything a settled preparation hands back to the actor's ordinary
/// pre-custody spawn ladder.
pub(super) type PreparedLocalSpawn = (
    ProfileName,
    AgentIdentity,
    ContentInput,
    Option<ContentInput>,
    crate::MobRuntimeMode,
    bool,
    std::collections::BTreeMap<String, String>,
    Option<MemberRef>,
    Option<SpawnProvisionInput>,
    Option<SessionId>,
    bool,
    Option<crate::profile::Profile>,
    Option<String>,
    Option<meerkat_core::interaction::ObjectiveId>,
    Option<Arc<dyn AgentToolDispatcher>>,
    AuthorizedSpawnProfileMaterial,
    super::super::handle::SpawnContinuityIntent,
    SpawnExecObservations,
);

/// The typed completion payload carried by
/// [`MobCommand::SpawnPreparationSettled`]. Opaque outside this module.
pub(in crate::runtime) struct SpawnPreparationOutcome(Result<PreparedLocalSpawn, MobError>);

pub(super) type SpawnPreparationFuture =
    ActorCommandFuture<'static, Result<PreparedLocalSpawn, MobError>>;

/// Pin a preparation future's output type, so the off-loop body can use `?`
/// and early returns exactly like the inline code it replaced.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn typed_preparation<F>(preparation: F) -> F
where
    F: std::future::Future<Output = Result<PreparedLocalSpawn, MobError>> + Send + 'static,
{
    preparation
}

/// Browser twin of [`typed_preparation`]: wasm32 session services are
/// `?Send`, and the single-threaded runtime spawns non-`Send` tasks.
#[cfg(target_arch = "wasm32")]
pub(super) fn typed_preparation<F>(preparation: F) -> F
where
    F: std::future::Future<Output = Result<PreparedLocalSpawn, MobError>> + 'static,
{
    preparation
}

/// Actor-held custody of one in-flight spawn preparation.
///
/// None of this travels to the preparation task: the reply, the respawn
/// origin, and the identity actuation permit stay in the actor's keyed table
/// so a lifecycle transition can settle them without the task.
pub(super) struct SpawnPreparationCarry {
    pub(super) requested_identity: AgentIdentity,
    pub(super) spawn_source: super::super::handle::SpawnSource,
    pub(super) identity_member_permit: Option<crate::identity::IdentityActuationPermit>,
    pub(super) respawn_origin: Option<RespawnOrigin>,
    pub(super) restore_wiring: Option<RestoreWiringPlan>,
    pub(super) reply_tx:
        oneshot::Sender<Result<super::super::handle::MemberSpawnReceipt, MobError>>,
    pub(super) suppress_autonomous_initial_prompt: bool,
    pub(super) spawned_by: Option<AgentIdentity>,
    pub(super) fork_job: Option<crate::runtime::ForkJobRecord>,
    pub(super) fork_source: Option<meerkat_core::ForkBuildSource>,
    pub(super) fork_overlay: super::super::ForkOverlayOrigin,
    pub(super) owner_bridge_session_id: Option<SessionId>,
    pub(super) ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
}

struct SpawnPreparationSlot {
    agent_identity: AgentIdentity,
    carry: SpawnPreparationCarry,
    task: tokio::task::JoinHandle<()>,
    started: Instant,
}

/// Keyed table of in-flight preparations plus their shared bound.
pub(in crate::runtime) struct SpawnPreparations {
    slots: BTreeMap<u64, SpawnPreparationSlot>,
    next_ticket: u64,
    permits: Arc<tokio::sync::Semaphore>,
}

impl SpawnPreparations {
    pub(in crate::runtime) fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
            next_ticket: 0,
            permits: Arc::new(tokio::sync::Semaphore::new(SPAWN_PREPARATION_CONCURRENCY)),
        }
    }

    pub(super) fn permits(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.permits)
    }

    #[cfg(test)]
    pub(super) fn in_flight(&self) -> usize {
        self.slots.len()
    }
}

impl Drop for SpawnPreparations {
    /// An exiting actor must not leak preparation tasks.
    fn drop(&mut self) {
        for slot in self.slots.values() {
            slot.task.abort();
        }
    }
}

/// Shared, immutable actor services a preparation reads through.
pub(super) struct LocalSpawnPreparationContext {
    pub(super) definition: Arc<MobDefinition>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) provisioner: Arc<dyn MobProvisioner>,
    pub(super) forked_participant_store: Option<Arc<dyn crate::store::ForkedParticipantStore>>,
    pub(super) runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    pub(super) tool_consequence_policy_registry:
        Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
}

impl LocalSpawnPreparationContext {
    pub(super) fn from_actor(actor: &MobActor) -> Self {
        Self {
            definition: Arc::clone(&actor.definition),
            session_service: Arc::clone(&actor.session_service),
            provisioner: Arc::clone(&actor.provisioner),
            forked_participant_store: actor.forked_participant_store.clone(),
            runtime_metadata: Arc::clone(&actor.runtime_metadata),
            tool_consequence_policy_registry: actor.tool_consequence_policy_registry.clone(),
            default_llm_client: actor.default_llm_client.clone(),
        }
    }
}

#[cfg(test)]
pub(super) static SPAWN_PREPARATION_TEST_GATES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<AgentIdentity, Arc<tokio::sync::Semaphore>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

impl MobActor {
    /// Whether a preparation for `agent_identity` is still running off the
    /// loop. Such an identity is owned exactly like a staged pending spawn.
    pub(super) fn spawn_preparation_in_flight(&self, agent_identity: &AgentIdentity) -> bool {
        self.spawn_preparations
            .slots
            .values()
            .any(|slot| &slot.agent_identity == agent_identity)
    }

    /// Hand one preparation to a supervised, bounded task. The actor keeps
    /// `carry`; the task reports exactly one typed completion.
    pub(super) fn start_spawn_preparation(
        &mut self,
        agent_identity: AgentIdentity,
        carry: SpawnPreparationCarry,
        preparation: SpawnPreparationFuture,
    ) {
        let ticket = self.spawn_preparations.next_ticket;
        self.spawn_preparations.next_ticket = ticket.wrapping_add(1);
        let permits = self.spawn_preparations.permits();
        let command_tx = self.command_tx.clone();
        let panic_log_ledger = Arc::clone(&self.spawn_panic_log_ledger);
        let mob_id = self.definition.id.clone();
        let task_identity = agent_identity.clone();
        #[cfg(test)]
        let test_gate = SPAWN_PREPARATION_TEST_GATES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&agent_identity)
            .cloned();
        let task = tokio::spawn(async move {
            // The permit is awaited here, off the actor loop. The semaphore
            // is never closed, so a failed acquire cannot occur.
            let permit = permits.acquire_owned().await.ok();
            #[cfg(test)]
            if let Some(gate) = test_gate {
                let _released = gate.acquire_owned().await;
            }
            let result = super::super::panic_capture::run_spawn_provision_guarded(
                panic_log_ledger.as_ref(),
                &mob_id,
                "spawn preparation task",
                &task_identity,
                preparation,
            )
            .await;
            drop(permit);
            // A closed channel means the actor is gone; its keyed table (and
            // the custody in it) went with it.
            let _ = command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::SpawnPreparationSettled {
                        ticket,
                        outcome: Box::new(SpawnPreparationOutcome(result)),
                    },
                ))
                .await;
        });
        self.spawn_preparations.slots.insert(
            ticket,
            SpawnPreparationSlot {
                agent_identity,
                carry,
                task,
                started: Instant::now(),
            },
        );
    }

    /// Typed completion of one preparation.
    pub(super) async fn spawn_preparation_settled(
        &mut self,
        ticket: u64,
        outcome: SpawnPreparationOutcome,
    ) {
        let SpawnPreparationOutcome(result) = outcome;
        let Some(slot) = self.spawn_preparations.slots.remove(&ticket) else {
            // A lifecycle transition already cancelled this preparation and
            // settled its custody.
            tracing::debug!(ticket, "spawn preparation settled after cancellation");
            return;
        };
        let preparation_ms = slot.started.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => tracing::debug!(
                ticket,
                agent_identity = %slot.agent_identity,
                preparation_ms,
                "spawn preparation completed off the actor loop"
            ),
            Err(error) => tracing::debug!(
                ticket,
                agent_identity = %slot.agent_identity,
                preparation_ms,
                error = %error,
                "spawn preparation failed off the actor loop"
            ),
        }
        Box::pin(self.finish_local_spawn_preparation(slot.carry, result)).await;
    }

    /// Cancel every in-flight preparation for a lifecycle transition and
    /// settle its custody (respawn-topology abandonment, identity reconcile
    /// disposition, reply) with a typed cancellation.
    pub(super) async fn cancel_spawn_preparations(&mut self, reason: &str) {
        for (ticket, slot) in std::mem::take(&mut self.spawn_preparations.slots) {
            slot.task.abort();
            let _ = slot.task.await;
            tracing::debug!(
                ticket,
                agent_identity = %slot.agent_identity,
                reason,
                "cancelled in-flight spawn preparation for lifecycle transition"
            );
            let error = MobError::Internal(format!(
                "spawn canceled for '{}': {reason}",
                slot.agent_identity
            ));
            Box::pin(self.finish_local_spawn_preparation(slot.carry, Err(error))).await;
        }
    }
}
