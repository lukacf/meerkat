//! Host-owned composition of native job and continuation delivery.

use std::sync::{Arc, RwLock};

use super::{MobSessionService, PreBuildMobSessionService, no_op_pre_build_hook};

/// Why a host could not compose or start delivery. Native refusals retain
/// their types so callers can distinguish acquisition, binding and ownership.
#[derive(Debug)]
pub enum MobRuntimeDeliveryError {
    MissingComposition,
    MissingRuntime,
    MissingBinding,
    RuntimeStoreMismatch,
    CompositionChanged,
    Runtime(meerkat_runtime::RuntimeDriverError),
    Binding(meerkat_mob_mcp::BindContinuationsError),
    AlreadyArmed(meerkat_runtime::RuntimeDeliveryOwnerAlreadyArmed),
}

impl std::fmt::Display for MobRuntimeDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingComposition => {
                f.write_str("agent mob tools require an explicit runtime delivery composition")
            }
            Self::MissingRuntime => {
                f.write_str("runtime delivery requires an acquired runtime machine")
            }
            Self::MissingBinding => {
                f.write_str("the replaced agent mob state has no continuation binding")
            }
            Self::RuntimeStoreMismatch => f.write_str(
                "runtime delivery and the acquired machine do not share runtime store authority",
            ),
            Self::CompositionChanged => f.write_str(
                "runtime delivery composition changed after agent mob tools were installed",
            ),
            Self::Runtime(error) => error.fmt(f),
            Self::Binding(error) => error.fmt(f),
            Self::AlreadyArmed(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for MobRuntimeDeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::Binding(error) => Some(error),
            Self::AlreadyArmed(error) => Some(error),
            _ => None,
        }
    }
}

impl From<meerkat_runtime::RuntimeDriverError> for MobRuntimeDeliveryError {
    fn from(error: meerkat_runtime::RuntimeDriverError) -> Self {
        Self::Runtime(error)
    }
}
impl From<meerkat_mob_mcp::BindContinuationsError> for MobRuntimeDeliveryError {
    fn from(error: meerkat_mob_mcp::BindContinuationsError) -> Self {
        Self::Binding(error)
    }
}
impl From<meerkat_runtime::RuntimeDeliveryOwnerAlreadyArmed> for MobRuntimeDeliveryError {
    fn from(error: meerkat_runtime::RuntimeDeliveryOwnerAlreadyArmed) -> Self {
        Self::AlreadyArmed(error)
    }
}

/// One host's inbox, continuation bindings and unarmed native delivery owner.
/// Clones share the inbox commit signal and binding slot. Arming two clones
/// concurrently is refused by the native inbox owner.
#[derive(Clone)]
pub struct MobRuntimeDelivery {
    inbox: meerkat_runtime::RuntimeDeliveryInbox,
    store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    bindings: Arc<meerkat::ContinuationHostBindings>,
    owner: meerkat::RuntimeDeliveryOwner,
    observation: Arc<RwLock<Option<DeliveryObservation>>>,
    /// Set when a candidate launch's preparation decided not to arm the
    /// owner, and no owner was live then; an authoritative arm clears it.
    /// See [`Self::is_quiesced_for_candidate`].
    quiesced_candidate: Arc<std::sync::atomic::AtomicBool>,
    /// The host's own committed-result owner, composed into the job source
    /// the agent mob-tool state binds.
    host_job_source: Option<meerkat_mob_mcp::HostJobSource>,
    /// The approval owner whose consent notices the delivery owner projects
    /// into this inbox; `None`, the default, projects none.
    consent_notice_issuer: Option<Arc<meerkat_core::ApprovalService>>,
}

#[derive(Clone)]
struct DeliveryObservation {
    passes: tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass>,
    running: std::sync::Weak<RunningMobRuntimeDelivery>,
}

impl MobRuntimeDelivery {
    /// Compose over the exact runtime-store facade and detached-job store
    /// installed on the session service and its FactoryAgentBuilder. This
    /// creates the inbox once; all producers must use [`Self::inbox`].
    pub fn new(
        store: Arc<dyn meerkat_runtime::RuntimeStore>,
        jobs: Arc<dyn meerkat::DetachedJobStore>,
    ) -> Self {
        let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::clone(&store));
        Self::over(inbox, Some(store), jobs)
    }

    /// Only the stock truly ephemeral constructor declares separate memory
    /// delivery storage: its native machine intentionally has no store.
    pub(super) fn declared_ephemeral(jobs: Arc<dyn meerkat::DetachedJobStore>) -> Self {
        Self::over(
            meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
                meerkat_runtime::InMemoryRuntimeStore::new(),
            )),
            None,
            jobs,
        )
    }

    fn over(
        inbox: meerkat_runtime::RuntimeDeliveryInbox,
        store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
        jobs: Arc<dyn meerkat::DetachedJobStore>,
    ) -> Self {
        Self {
            owner: meerkat::RuntimeDeliveryOwner::new(jobs, inbox.clone()),
            inbox,
            store,
            bindings: Arc::default(),
            observation: Arc::default(),
            quiesced_candidate: Arc::default(),
            host_job_source: None,
            consent_notice_issuer: None,
        }
    }

    /// Compose the host's own committed-result owner, for exactly its
    /// declared `Host` namespaces, into the job source the agent mob-tool
    /// state binds over this delivery: the same delivery owner confirms the
    /// host's retained results, its fork_off and council completions, and
    /// restored rows from the first pass. The host submits its results
    /// through the bound state's `continuation_service()`.
    #[must_use]
    pub fn with_host_job_source(mut self, host: meerkat_mob_mcp::HostJobSource) -> Self {
        self.host_job_source = Some(host);
        self
    }

    pub(super) fn host_job_source(&self) -> Option<meerkat_mob_mcp::HostJobSource> {
        self.host_job_source.clone()
    }

    /// Project the consent notices `issuer` owes this runtime's sessions:
    /// once armed, the delivery owner commits each notice that approval owner
    /// owes the hosting domain of this inbox's store into this inbox, and
    /// confirms it against that owner when a governed runtime admits it
    /// (Meerkat's `ConsentNoticeProjector`). It also installs the runtime's
    /// runless-terminal hint on this delivery's bindings, so a parked consent
    /// whose admitted notice input ended without a run is settled by the next
    /// round. The gateway passes exactly the issuer of the host's review
    /// (`BoundOperationReview::consent_notice_issuer`). A store without a
    /// hosting domain scopes no consent, so nothing is owed there and nothing
    /// is composed. Without an issuer, the default, no consent notice is
    /// projected.
    #[must_use]
    pub(crate) fn with_consent_notice_issuer(
        mut self,
        issuer: Arc<meerkat_core::ApprovalService>,
    ) -> Self {
        self.consent_notice_issuer = Some(issuer);
        self
    }

    /// The projector of the consent notices [`Self::with_consent_notice_issuer`]
    /// names, into this inbox, for the hosting domain of its store: the
    /// domain the runtime scopes consent to. `None` without an issuer, and on
    /// a store without a hosting domain.
    fn consent_notice_projector(
        &self,
        runtime: &Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Option<Arc<meerkat::ConsentNoticeProjector>> {
        let issuer = self.consent_notice_issuer.as_ref()?;
        let domain = self.inbox.hosting_capability().domain_ref()?;
        Some(Arc::new(meerkat::ConsentNoticeProjector::new(
            Arc::clone(issuer),
            domain,
            self.inbox(),
            runtime,
        )))
    }

    /// Clone the host's inbox, including its commit signal.
    pub fn inbox(&self) -> meerkat_runtime::RuntimeDeliveryInbox {
        self.inbox.clone()
    }

    /// Latest completed native pass. `None` means the owner has not armed or
    /// has not yet completed its initial reconciliation.
    pub fn last_pass(&self) -> Option<meerkat::RuntimeDeliveryPass> {
        let observation = self
            .observation
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pass = observation.as_ref()?.passes.borrow().clone();
        (pass.generation > 0).then_some(pass)
    }

    /// Whether an armed owner still runs. This is lifecycle evidence, not a
    /// claim that queued deliveries have drained.
    pub fn is_running(&self) -> bool {
        self.observation
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|value| value.running.upgrade())
            .is_some_and(|running| !running.is_stopped())
    }

    /// Whether the owner is unarmed because a candidate launch composed it:
    /// the arming decision itself, recorded when it was made, never an
    /// inference from a missing pass. An owner that runs, that was never
    /// armed for another reason, or that stopped, is not quiesced.
    pub(crate) fn is_quiesced_for_candidate(&self) -> bool {
        self.quiesced_candidate
            .load(std::sync::atomic::Ordering::SeqCst)
            && !self.is_running()
    }

    pub(super) fn shares_composition(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bindings, &other.bindings)
    }

    /// Reject an already running owner before bootstrap creates a mob. This
    /// is only a preflight; native arm remains the final ownership authority.
    pub(super) fn preflight(&self) -> Result<(), MobRuntimeDeliveryError> {
        drop(self.inbox.claim_delivery_ownership()?);
        Ok(())
    }

    pub(super) fn validate_runtime(
        &self,
        runtime: Option<&meerkat_runtime::MeerkatMachine>,
    ) -> Result<(), MobRuntimeDeliveryError> {
        let runtime = runtime.ok_or(MobRuntimeDeliveryError::MissingRuntime)?;
        let matches = match &self.store {
            Some(store) => runtime.shares_runtime_store_authority(store),
            None => !runtime.has_runtime_persistence(),
        };
        if matches {
            Ok(())
        } else {
            Err(MobRuntimeDeliveryError::RuntimeStoreMismatch)
        }
    }

    pub(super) fn bind(
        &self,
        state: &Arc<meerkat_mob_mcp::MobMcpState>,
        replaced: Option<meerkat::ContinuationBindingGeneration>,
    ) -> Result<(), MobRuntimeDeliveryError> {
        match replaced {
            Some(generation) => {
                state.rebind_continuations(generation, self.inbox(), &self.bindings)?;
            }
            None => state.bind_continuations(self.inbox(), &self.bindings)?,
        }
        Ok(())
    }

    pub(super) fn arm(
        &self,
        service: Arc<dyn MobSessionService>,
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Result<Arc<RunningMobRuntimeDelivery>, MobRuntimeDeliveryError> {
        self.validate_runtime(Some(&runtime))?;
        // The native generic host needs a sized service. This forwarding
        // facade contains the final composed service, including every host
        // wrapper, and adds no delivery semantics.
        let service = Arc::new(PreBuildMobSessionService {
            inner: service,
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: Some(Arc::clone(&runtime)),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let host = meerkat::surface::SessionServiceDeliveryHost::new(
            &service,
            &runtime,
            meerkat::DetachedJobService::new(self.owner.job_store()),
            None,
        )
        .with_continuation_bindings(Arc::clone(&self.bindings));
        // The same owner, over the same inbox, also projects the consent
        // notices the host's issuer owes, when there is one.
        let host = Arc::new(match self.consent_notice_projector(&runtime) {
            Some(projector) => {
                // A round that settles a parked consent whose admitted notice
                // input ended without a run starts on the runtime's runless
                // terminal hint, routed into the source changes this owner
                // subscribes to. The hint holds these bindings weakly.
                self.bindings.announce_runless_input_terminals_of(&runtime);
                host.with_consent_notices(projector)
            }
            None => host,
        });
        let handle = self
            .owner
            .clone()
            .with_attachment_commits(runtime.subscribe_attachment_commits())
            .with_run_settlements(runtime.subscribe_run_settlements())
            // Subscribed before arming, as the attachment and settlement
            // signals: an announcement after the owner exists is never lost.
            .with_source_changes(self.bindings.subscribe_source_changes())
            .arm(host)?;
        let passes = handle.subscribe_passes();
        let running = Arc::new(RunningMobRuntimeDelivery {
            passes: passes.clone(),
            handle: std::sync::Mutex::new(Some(handle)),
            bindings: Arc::downgrade(&self.bindings),
            _service: service,
            _runtime: runtime,
        });
        *self
            .observation
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(DeliveryObservation {
            passes,
            running: Arc::downgrade(&running),
        });
        Ok(running)
    }

    /// Arm the native owner for a launch that speaks for the durable
    /// composition, exactly as [`Self::arm`] does. A launch that does not (a
    /// candidate) arms nothing: its owner would admit the durable inbox it
    /// shares with the authoritative launch (continuation deliveries such as
    /// announcement batches, fork and council results, detached-job
    /// terminals and consent notices) into sessions it has no authority
    /// over, a second time. The decision is recorded as this delivery's
    /// owner state; the inbox, its bindings and every pending row stay for
    /// the authoritative launch.
    pub(super) fn arm_for_launch(
        &self,
        authority: crate::mob_composition_manifest::CompositionAuthority,
        service: Arc<dyn MobSessionService>,
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Result<Option<Arc<RunningMobRuntimeDelivery>>, MobRuntimeDeliveryError> {
        if authority.speaks_for_composition() {
            let running = self.arm(service, runtime)?;
            // A live owner is never a candidate's.
            self.quiesced_candidate
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Ok(Some(running));
        }
        self.validate_runtime(Some(&runtime))?;
        // Clones share this state, and a candidate's preparation can finish
        // after an authoritative launch over the same composition armed:
        // decided under the observation an arm publishes its owner in, a
        // candidate never marks a live owner quiesced.
        let observation = self
            .observation
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let live_owner = observation
            .as_ref()
            .and_then(|value| value.running.upgrade())
            .is_some_and(|running| !running.is_stopped());
        if !live_owner {
            self.quiesced_candidate
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        drop(observation);
        tracing::info!(
            "runtime delivery owner not armed: this launch is a candidate, which does not speak \
             for the durable composition; pending deliveries stay in the durable inbox for the \
             authoritative launch, and durable monitor recovery is skipped"
        );
        Ok(None)
    }
}

/// Retained by all clones of the live MobRuntime. [`Self::stop`] stops the
/// native owner explicitly when the mob stops for teardown; otherwise the
/// last clone drops the native handle, which stops its owner and releases
/// delivery ownership.
pub(super) struct RunningMobRuntimeDelivery {
    passes: tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass>,
    handle: std::sync::Mutex<Option<meerkat::RuntimeDeliveryOwnerHandle>>,
    /// The continuation bindings the owner was armed over, held weakly for
    /// the host's [`ContinuationSourceAnnouncer`] only: it keeps neither
    /// them nor the owner alive.
    bindings: std::sync::Weak<meerkat::ContinuationHostBindings>,
    _service: Arc<PreBuildMobSessionService>,
    _runtime: Arc<meerkat_runtime::MeerkatMachine>,
}

impl RunningMobRuntimeDelivery {
    pub(super) fn last_pass(&self) -> meerkat::RuntimeDeliveryPass {
        self.passes.borrow().clone()
    }

    pub(super) fn source_announcer(&self) -> ContinuationSourceAnnouncer {
        ContinuationSourceAnnouncer {
            bindings: self.bindings.clone(),
        }
    }

    /// Stop the native owner now, even while clones of the runtime live:
    /// a stopped mob must not keep applying deliveries. Undelivered rows stay
    /// pending in the durable inbox.
    pub(super) fn stop(&self) {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(handle);
    }

    /// Whether the native owner's task has ended: its pass channel closes
    /// with it.
    #[cfg(test)]
    pub(super) fn owner_task_ended(&self) -> bool {
        self.passes.has_changed().is_err()
    }

    /// Retire the native owner and wait for its task, at most `bound`: no
    /// pass starts after the request, and one already running (blocking
    /// store work included) finishes first. Once the task ended, the handle
    /// goes as [`Self::stop`] drops it; an owner still running at the bound
    /// keeps its handle, so a later retirement joins the same task. An owner
    /// already stopped (its handle gone) answers `Ok` at once, unjoined.
    pub(super) async fn retire(
        &self,
        bound: std::time::Duration,
    ) -> Result<(), meerkat::RuntimeDeliveryOwnerRetirementIncomplete> {
        let retirement = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(meerkat::RuntimeDeliveryOwnerHandle::retire);
        if let Some(retirement) = retirement {
            // A notification registration the retirement left with the
            // process never reads or writes the store, so the retirement is
            // complete; the join traced its directory.
            let _retired = retirement.join(bound).await?;
        }
        self.stop();
        Ok(())
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_none_or(meerkat::RuntimeDeliveryOwnerHandle::is_stopped)
    }
}

/// Tells a runtime's continuation delivery that the custody its bound
/// continuation sources read may have changed. A host calls it once it has
/// restored that custody, and once its source owner settled an original a
/// retained resume may wait on (it promoted or withdrew a candidate whose
/// retained custody read `AdmissionPending`), after the facts are readable
/// through the bound source. The delivery owner subscribed to the same
/// bindings then retries the rows it holds blocked (see
/// [`meerkat::ContinuationHostBindings::source_custody_changed`]).
///
/// It is a hint, not a fact. It grants nothing, asserts nothing and carries
/// no source truth: the retry re-reads current custody and permissions, and
/// a row they still refuse stays where it is. Clones share one weak handle
/// to the bindings: holding one keeps neither the bindings nor the delivery
/// owner alive, and it exposes no binding, rebinding or other authority.
/// Nothing here waits for the owner to be ready.
#[derive(Clone, Debug)]
pub struct ContinuationSourceAnnouncer {
    bindings: std::sync::Weak<meerkat::ContinuationHostBindings>,
}

impl ContinuationSourceAnnouncer {
    /// Publish the hint to the delivery owner subscribed to these bindings.
    ///
    /// `true` means only that the bindings still existed and the hint was
    /// published. It does not mean the delivery owner is running, that the
    /// source custody is live, or that any row was admitted, settled or
    /// retried successfully: the bindings can outlive an explicitly stopped
    /// owner (a setup clone of the delivery composition keeps them), and
    /// then nothing acts on the hint. `false` means the bindings are gone.
    /// Neither answer is a delivery outcome: a caller must not replay,
    /// replace or fail a turn because of it.
    pub fn announce_source_custody_changed(&self) -> bool {
        match self.bindings.upgrade() {
            Some(bindings) => {
                bindings.source_custody_changed();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::mob_handle_runtime::{MobBootstrapSpec, MobRuntime, MobRuntimeError};

    struct Fixture {
        spec: MobBootstrapSpec,
        delivery: MobRuntimeDelivery,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        jobs: Arc<dyn meerkat::DetachedJobStore>,
    }

    fn fixture(path: &std::path::Path, store: Arc<dyn meerkat_runtime::RuntimeStore>) -> Fixture {
        fixture_with_issuer(path, store, None)
    }

    /// [`fixture`] whose delivery projects the consent notices `issuer` owes.
    fn fixture_with_issuer(
        path: &std::path::Path,
        store: Arc<dyn meerkat_runtime::RuntimeStore>,
        issuer: Option<Arc<meerkat_core::ApprovalService>>,
    ) -> Fixture {
        let blobs: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
                crate::blob_store::ObjectStoreBlobStore::memory(),
            )));
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(Arc::clone(&store), Arc::clone(&blobs))
                .expect("machine"),
        );
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        let mut builder = meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::new(path).builtins(false).comms(true),
            meerkat::Config::default(),
        );
        builder.default_detached_job_store = Some(Arc::clone(&jobs));
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            Arc::new(meerkat::MemoryStore::new()),
            Arc::clone(&store),
            blobs,
        ));
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"delivery-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4()
        ))
        .expect("definition");
        let delivery = MobRuntimeDelivery::new(store, Arc::clone(&jobs));
        let delivery = match issuer {
            Some(issuer) => delivery.with_consent_notice_issuer(issuer),
            None => delivery,
        };
        let spec = MobBootstrapSpec::new(definition, meerkat_mob::MobStorage::in_memory(), service)
            .with_session_runtime_adapter(Arc::clone(&machine))
            .expect("acquire owner")
            .with_runtime_delivery(delivery.clone());
        Fixture {
            spec,
            delivery,
            machine,
            jobs,
        }
    }

    fn memory_fixture(path: &std::path::Path) -> Fixture {
        fixture(path, Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()))
    }

    /// A SQLite runtime store in `dir` that hosts its sessions across
    /// processes, so it has a hosting domain the runtime scopes consent to.
    fn hosted_store(dir: &std::path::Path) -> Arc<dyn meerkat_runtime::RuntimeStore> {
        let locks = dir.join("session-hosting");
        std::fs::create_dir_all(&locks).expect("hosting lock directory");
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(dir.join("runtime.sqlite"))
                .expect("open")
                .with_hosting_paths(meerkat_runtime::HostingPaths {
                    hosting_lock_dir: locks,
                    cold_delivery_lock: dir.join("cold-delivery.lock"),
                    database: None,
                }),
        )
    }

    /// The hosting domain of `delivery`'s inbox store.
    fn hosting_domain(
        delivery: &MobRuntimeDelivery,
    ) -> meerkat_core::session_hosting::HostingDomainRef {
        delivery
            .inbox()
            .hosting_capability()
            .domain_ref()
            .expect("the store hosts its sessions across processes")
    }

    /// One consent `approvals` issued for `session` in `domain`, approved by
    /// an authenticated human and parked by its run's close, through the
    /// approval owner's own paths: the owner owes its approved notice to that
    /// session.
    async fn parked_consent(
        approvals: &meerkat_core::ApprovalService,
        domain: meerkat_core::session_hosting::HostingDomainRef,
        session: &meerkat_core::types::SessionId,
    ) -> meerkat_core::approval::consent_test_support::ParkedRoutedConsent {
        let parked = meerkat_core::approval::consent_test_support::ParkedRoutedConsent::park(
            approvals,
            meerkat_core::ApprovalConsentScope {
                domain,
                session_id: session.clone(),
            },
            meerkat_core::retained_work::RetainedWorkIdentity::new(
                "consent-origin",
                meerkat_core::RunId::new(),
                Vec::new(),
                std::collections::BTreeMap::new(),
                None,
            ),
        )
        .await
        .expect("a parked routed consent");
        assert_eq!(
            approvals.owed_consent_deliveries().expect("owed").owed,
            vec![(
                parked.approval_id().clone(),
                meerkat_core::ApprovalConsentTransition::Approved
            )],
            "the approved notice is owed"
        );
        parked
    }

    /// The continuation status of the approved notice of `approval` owed to
    /// `session`, read through `delivery`'s inbox.
    async fn approved_notice_status(
        delivery: &MobRuntimeDelivery,
        machine: &Arc<meerkat_runtime::MeerkatMachine>,
        session: &meerkat_core::types::SessionId,
        approval: &meerkat_core::ApprovalId,
    ) -> meerkat::ContinuationStatus {
        notice_status(
            delivery,
            machine,
            session,
            approval,
            meerkat_core::ApprovalConsentTransition::Approved,
        )
        .await
    }

    /// How long the consent rows wait for an owner event before failing.
    const EVENT_GUARD: std::time::Duration = std::time::Duration::from_secs(10);
    /// How long an owner must publish no pass to count as settled.
    const QUIET: std::time::Duration = std::time::Duration::from_millis(300);

    /// The passes of `delivery`'s armed owner, as they are published.
    fn owner_passes(
        delivery: &MobRuntimeDelivery,
    ) -> tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass> {
        delivery
            .observation
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .expect("an armed owner")
            .passes
            .clone()
    }

    /// Wait until the owner publishes no pass for [`QUIET`], within
    /// [`EVENT_GUARD`].
    async fn owner_settled(
        passes: &mut tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass>,
    ) {
        tokio::time::timeout(EVENT_GUARD, async {
            while let Ok(changed) = tokio::time::timeout(QUIET, passes.changed()).await {
                changed.expect("owner pass channel open");
            }
        })
        .await
        .expect("the owner settles");
    }

    /// Where the `transition` notice of `approval` stands at its approval
    /// owner, if the transition committed.
    fn delivery_state(
        approvals: &meerkat_core::ApprovalService,
        approval: &meerkat_core::ApprovalId,
        transition: meerkat_core::ApprovalConsentTransition,
    ) -> Option<meerkat_core::ApprovalConsentDeliveryState> {
        approvals
            .consent_delivery(approval, transition)
            .expect("the approval owner reads its notice")
            .map(|facts| facts.state)
    }

    /// The terminal the runtime reports for `input` of `session`, if any.
    async fn input_terminal(
        machine: &meerkat_runtime::MeerkatMachine,
        session: &meerkat_core::types::SessionId,
        input: &meerkat_core::lifecycle::InputId,
    ) -> Option<meerkat_runtime::input_state::InputTerminalOutcome> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        machine
            .interaction_terminal_status(
                session,
                meerkat_runtime::terminal_status::InteractionSelector::InputId(input.clone()),
            )
            .await
            .expect("the runtime reads its input")
            .expect("the runtime knows its input")
            .report
            .terminal
    }

    /// The continuation status of the `transition` notice of `approval` owed
    /// to `session`, read through `delivery`'s inbox.
    async fn notice_status(
        delivery: &MobRuntimeDelivery,
        machine: &Arc<meerkat_runtime::MeerkatMachine>,
        session: &meerkat_core::types::SessionId,
        approval: &meerkat_core::ApprovalId,
        transition: meerkat_core::ApprovalConsentTransition,
    ) -> meerkat::ContinuationStatus {
        let job_id = meerkat_core::approval::consent::consent_delivery_job_id(approval, transition);
        meerkat::ContinuationOwnerService::new(
            delivery.inbox(),
            Arc::new(meerkat::SessionAddressResolver),
            Arc::clone(machine),
        )
        .continuation_status(
            &meerkat::ContinuationOwner::Session {
                session_id: session.clone(),
            },
            // The projector keys a notice's continuation by its job id.
            &meerkat::ContinuationKey::new(format!("consent:{job_id}")).expect("key"),
        )
        .await
        .expect("status")
    }

    async fn first_pass(delivery: &MobRuntimeDelivery) -> meerkat::RuntimeDeliveryPass {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(pass) = delivery.last_pass() {
                    break pass;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("initial native owner pass")
    }

    async fn owner_released(delivery: &MobRuntimeDelivery) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if delivery.preflight().is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("native task releases inbox ownership after guard drop");
    }

    #[tokio::test]
    async fn custom_tools_require_delivery_without_publishing_a_factory() {
        let dir = tempfile::tempdir().expect("temp");
        let mut fixture = memory_fixture(dir.path());
        fixture.spec.runtime_delivery = None;
        let slot = Arc::new(std::sync::RwLock::new(None));
        assert!(matches!(
            fixture.spec.with_agent_mob_tools(Arc::clone(&slot)),
            Err(MobRuntimeDeliveryError::MissingComposition)
        ));
        assert!(slot.read().expect("slot").is_none());
    }

    #[tokio::test]
    async fn replacing_delivery_after_tool_install_refuses_before_mob_creation() {
        let dir = tempfile::tempdir().expect("temp");
        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let fixture = fixture(dir.path(), Arc::clone(&store));
        let spec = fixture
            .spec
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("tools");
        let storage = spec.storage.clone();
        let spec = spec.with_runtime_delivery(MobRuntimeDelivery::new(store, fixture.jobs));
        assert!(matches!(
            MobRuntime::prepare(spec).await,
            Err(MobRuntimeError::Delivery(
                MobRuntimeDeliveryError::CompositionChanged
            ))
        ));
        assert!(storage.is_event_log_empty().await.expect("events"));
    }

    /// A host's own committed-result owner composed into the spec's
    /// SELECTED delivery after the constructor already bound the agent
    /// mob-tool state: bootstrap rebuilds and rebinds the state, so the bound
    /// job source answers the host's declared namespace (and only it), and
    /// the bound state exposes the service the host submits through.
    #[tokio::test]
    async fn a_host_job_source_added_after_binding_joins_the_bound_source_at_bootstrap() {
        struct SetupStore(meerkat::RetainedJobFacts);
        #[async_trait::async_trait]
        impl meerkat::RetainedJobSource for SetupStore {
            async fn retained_job(
                &self,
                _producer: &meerkat::ContinuationProducer,
                _job_id: &str,
            ) -> meerkat::RetainedJobLookup {
                meerkat::RetainedJobLookup::Found(self.0.clone())
            }
        }
        let host_producer = |namespace: &str| meerkat::ContinuationProducer::Host {
            namespace: namespace.into(),
        };
        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let facts = meerkat::RetainedJobFacts {
            owner_session_id: meerkat_core::types::SessionId::new(),
            retained_work: None,
            result_digest: "sha256:setup".into(),
        };
        let host = meerkat_mob_mcp::HostJobSource::new(
            ["toolkit.setup"],
            Arc::new(SetupStore(facts.clone())),
        )
        .expect("a named namespace");

        // A spec without a selected delivery refuses; it never builds one.
        let mut bare = memory_fixture(dir.path()).spec;
        bare.runtime_delivery = None;
        assert!(matches!(
            bare.with_host_job_source(host.clone()),
            Err(MobRuntimeDeliveryError::MissingComposition)
        ));

        let spec = fixture
            .spec
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("tools");
        let bound_before = spec
            .agent_mob_mcp_state
            .as_ref()
            .and_then(|state| state.continuation_binding_generation())
            .expect("the constructor bound the state");
        let before = fixture
            .delivery
            .bindings
            .job_source()
            .expect("a live binding");
        assert_eq!(
            before
                .retained_job(&host_producer("toolkit.setup"), "setup-1")
                .await,
            meerkat::RetainedJobLookup::Absent,
            "the first binding knows no host namespace"
        );
        drop(before);

        let spec = spec
            .with_host_job_source(host)
            .expect("a selected delivery");
        let (runtime, pending) = MobRuntime::prepare(spec)
            .await
            .expect("prepare with the host's job source");
        assert!(pending.is_none());
        let state = runtime.agent_mob_mcp_state.as_ref().expect("bound state");
        assert_ne!(
            state.continuation_binding_generation(),
            Some(bound_before),
            "bootstrap rebound the state"
        );
        assert!(
            state.continuation_service().is_some(),
            "the host submits through the bound service"
        );
        let bound = fixture
            .delivery
            .bindings
            .job_source()
            .expect("a live binding");
        assert_eq!(
            bound
                .retained_job(&host_producer("toolkit.setup"), "setup-1")
                .await,
            meerkat::RetainedJobLookup::Found(facts.clone())
        );
        assert_eq!(
            bound
                .retained_job(&host_producer("toolkit.other"), "setup-1")
                .await,
            meerkat::RetainedJobLookup::Absent
        );

        // The host commits its result through the bound service, confirmed
        // by the composed source: the record is read from that source.
        let record = meerkat::RetainedJobRecord::from_owner(
            &*bound,
            host_producer("toolkit.setup"),
            "setup-1",
        )
        .await
        .expect("read the composed source")
        .expect("the setup result is committed");
        let owner = meerkat::ContinuationOwner::Session {
            session_id: facts.owner_session_id.clone(),
        };
        let service = state.continuation_service().expect("bound service");
        let delivery = meerkat::ContinuationDelivery {
            key: meerkat::ContinuationKey::new("setup:setup-1").expect("key"),
            result: meerkat::ContinuationResultRef {
                producer: host_producer("toolkit.setup"),
                producer_id: "setup-1".into(),
                result_digest: facts.result_digest.clone(),
                summary: None,
            },
            body: "setup finished".into(),
            handling: meerkat::ContinuationHandling::Queue,
        };
        let receipt = service
            .submit_retained_completion(&owner, delivery.clone(), &record, 1)
            .await
            .expect("commit the host's retained result");
        assert_eq!(
            service
                .submit_retained_completion(&owner, delivery, &record, 2)
                .await
                .expect("replay"),
            receipt,
            "a replay returns the first receipt"
        );
        runtime.handle().shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn child_policy_reinstall_rebinds_exact_generation_with_old_state_alive() {
        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let spec = fixture
            .spec
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("tools");
        let old = spec.agent_mob_mcp_state.clone().expect("old state");
        let previous = old
            .continuation_binding_generation()
            .expect("old generation");
        let spec = spec.with_child_application_tool_policy(
            meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
        );
        let (runtime, pending) = MobRuntime::prepare(spec)
            .await
            .expect("prepare after policy replacement");
        assert!(pending.is_none());
        let current = runtime.agent_mob_mcp_state.as_ref().expect("new state");
        assert!(!Arc::ptr_eq(&old, current));
        assert_ne!(current.continuation_binding_generation(), Some(previous));
        assert_eq!(current.detached_delivery_blocked_because(), None);
        let stale = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            current.session_service(),
            Some(Arc::clone(&fixture.machine)),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("state")
        .into_shared();
        assert!(matches!(
            fixture.delivery.bind(&stale, Some(previous)),
            Err(MobRuntimeDeliveryError::Binding(
                meerkat_mob_mcp::BindContinuationsError::Binding(
                    meerkat::ContinuationBindError::StaleGeneration { .. }
                )
            ))
        ));
        assert_ne!(current.continuation_binding_generation(), Some(previous));
        runtime.handle().shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn duplicate_owner_refuses_before_creating_a_mob_and_last_runtime_drop_releases_it() {
        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let service = Arc::clone(&fixture.spec.session_service);
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("first boot");
        first_pass(&fixture.delivery).await;
        let retained = runtime.clone();
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"duplicate-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4()
        ))
        .expect("definition");
        let storage = meerkat_mob::MobStorage::in_memory();
        let spec = MobBootstrapSpec::new(definition, storage.clone(), service)
            .with_session_runtime_adapter(Arc::clone(&fixture.machine))
            .expect("owner")
            .with_runtime_delivery(fixture.delivery.clone());
        assert!(matches!(
            MobRuntime::prepare(spec).await,
            Err(MobRuntimeError::Delivery(
                MobRuntimeDeliveryError::AlreadyArmed(_)
            ))
        ));
        assert!(
            storage
                .is_event_log_empty()
                .await
                .expect("no bootstrap effects")
        );
        drop(runtime);
        assert!(
            fixture.delivery.is_running(),
            "a MobRuntime clone retains delivery ownership"
        );
        retained.handle().shutdown().await.expect("shutdown");
        drop(retained);
        owner_released(&fixture.delivery).await;
        assert!(!fixture.delivery.is_running());
    }

    #[tokio::test]
    async fn stopping_runtime_delivery_releases_the_owner_while_clones_live() {
        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("first boot");
        first_pass(&fixture.delivery).await;
        let retained = runtime.clone();
        assert!(fixture.delivery.is_running());
        runtime.stop_runtime_delivery();
        owner_released(&fixture.delivery).await;
        assert!(
            !fixture.delivery.is_running(),
            "an explicit stop releases delivery ownership while a runtime clone lives"
        );
        assert!(
            retained.runtime_delivery_pass().is_some(),
            "the last observed pass stays readable after the stop"
        );
        retained.handle().shutdown().await.expect("shutdown");
    }

    /// A retirement whose bound passes keeps the owner's handle, and a later
    /// one joins that same task. The owner runs on its own single-threaded
    /// runtime, which nothing drives while the first retirement waits, so
    /// that zero bound always passes first. On the owner's runtime the stop
    /// request would wake the owner, which would end before the timer fired.
    #[test]
    fn a_retirement_past_its_bound_keeps_the_owner_for_the_next() {
        let owner_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("owner runtime");
        let waiter_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("waiter runtime");
        let _owner_context = owner_runtime.enter();
        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let runtime = owner_runtime
            .block_on(MobRuntime::bootstrap(fixture.spec))
            .expect("first boot");
        owner_runtime.block_on(first_pass(&fixture.delivery));
        let running = runtime.runtime_delivery.as_ref().expect("armed delivery");

        let first = waiter_runtime.block_on(running.retire(std::time::Duration::ZERO));
        assert_eq!(
            (
                first.map_err(|incomplete| incomplete.bound),
                running.is_stopped()
            ),
            (Err(std::time::Duration::ZERO), false),
            "a retirement past its bound keeps the owner's handle"
        );
        let second = owner_runtime.block_on(running.retire(std::time::Duration::from_secs(10)));
        assert_eq!(
            (
                second.map_err(|incomplete| incomplete.bound),
                running.is_stopped(),
                running.owner_task_ended()
            ),
            (Ok(()), true, true),
            "a later retirement joins the same owner"
        );
        owner_runtime
            .block_on(runtime.handle().shutdown())
            .expect("shutdown");
    }

    #[tokio::test]
    async fn restored_mobs_arm_delivery_before_deferred_identity_activation() {
        for identity_first in [false, true] {
            let dir = tempfile::tempdir().expect("temp");
            let fixture = memory_fixture(dir.path());
            let definition = fixture.spec.definition.clone();
            let mob_id = definition.id.clone();
            let storage = fixture.spec.storage.clone();
            let service = Arc::clone(&fixture.spec.session_service);
            let first = MobRuntime::bootstrap(fixture.spec)
                .await
                .expect("first boot");
            let stopped = first
                .handle()
                .stop()
                .await
                .expect("persist stopped lifecycle");
            assert!(stopped.members.is_empty(), "the fixture has no members");
            assert_eq!(
                first.handle().status().await.expect("stopped state"),
                meerkat_mob::MobState::Stopped
            );
            assert!(
                first
                    .handle()
                    .events()
                    .replay_all()
                    .await
                    .expect("replay durable stopped lifecycle")
                    .iter()
                    .any(|event| matches!(event.kind, meerkat_mob::MobEventKind::MobStopped)),
                "the fixture must record an explicit Stop before recovery"
            );
            first
                .handle()
                .shutdown()
                .await
                .expect("close stopped actor");
            drop(first);
            owner_released(&fixture.delivery).await;

            let mut spec = MobBootstrapSpec::new(definition, storage, service)
                .with_declared_ephemeral_mob_storage()
                .with_session_runtime_adapter(Arc::clone(&fixture.machine))
                .expect("same machine")
                .with_runtime_delivery(fixture.delivery.clone())
                .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
                .expect("install tools on restore");
            if !identity_first {
                spec.identity_runtime_slot = None;
            }
            let (runtime, pending) = MobRuntime::prepare(spec).await.expect("prepare restore");
            assert!(fixture.delivery.is_running(), "owner is armed on return");
            let state = runtime
                .agent_mob_mcp_state
                .as_ref()
                .expect("resolver state");
            let restored_state = state.mob_status(&mob_id).await.expect("inserted handle");
            if identity_first {
                assert_eq!(restored_state, meerkat_mob::MobState::Stopped);
                pending
                    .expect("identity composition retains activation")
                    .activate()
                    .await
                    .expect("consume deferred activation for this empty mob");
            } else {
                assert!(pending.is_none(), "classic composition consumes activation");
                assert_eq!(restored_state, meerkat_mob::MobState::Running);
            }
            assert_eq!(
                runtime.handle().status().await.expect("running state"),
                meerkat_mob::MobState::Running
            );
            runtime.handle().shutdown().await.expect("shutdown");
            drop(runtime);
            owner_released(&fixture.delivery).await;
        }
    }

    fn completion(key: &str) -> meerkat::ContinuationDelivery {
        meerkat::ContinuationDelivery {
            key: meerkat::ContinuationKey::new(key).expect("key"),
            result: meerkat::ContinuationResultRef {
                producer: meerkat::ContinuationProducer::Host {
                    namespace: "meta-test".to_string(),
                },
                producer_id: key.to_string(),
                result_digest: "sha256:completion".to_string(),
                summary: None,
            },
            body: "completed".into(),
            handling: meerkat::ContinuationHandling::Queue,
        }
    }

    #[tokio::test]
    async fn pending_completion_reopens_and_applies_once_through_the_composed_owner() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("runtime.sqlite3");
        let session = meerkat_core::SessionId::new();
        let owner = meerkat::ContinuationOwner::Session {
            session_id: session.clone(),
        };
        let receipt = {
            let store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("open"));
            let fixture = fixture(dir.path(), store);
            let continuations = meerkat::ContinuationOwnerService::new(
                fixture.delivery.inbox(),
                Arc::new(meerkat::SessionAddressResolver),
                Arc::clone(&fixture.machine),
            );
            let receipt = continuations
                .submit(&owner, completion("reopen"), 10)
                .await
                .expect("commit pending completion");
            let runtime = MobRuntime::bootstrap(fixture.spec)
                .await
                .expect("first boot");
            let pass = first_pass(&fixture.delivery).await;
            assert_eq!(pass.applied, 0);
            assert_eq!(
                fixture
                    .delivery
                    .inbox()
                    .pending_delivery_total()
                    .await
                    .expect("backlog"),
                1
            );
            runtime.handle().shutdown().await.expect("shutdown");
            drop(runtime);
            owner_released(&fixture.delivery).await;
            receipt
        };
        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("reopen"));
        let fixture = fixture(dir.path(), store);
        fixture
            .machine
            .register_session(session.clone())
            .await
            .expect("register recipient");
        let continuations = meerkat::ContinuationOwnerService::new(
            fixture.delivery.inbox(),
            Arc::new(meerkat::SessionAddressResolver),
            Arc::clone(&fixture.machine),
        );
        assert_eq!(
            continuations
                .submit(&owner, completion("reopen"), 99)
                .await
                .expect("idempotent reopen"),
            receipt
        );
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("second boot");
        let pass = first_pass(&fixture.delivery).await;
        assert_eq!(pass.applied, 1, "{pass:?}");
        let applied = continuations
            .continuation_status(
                &owner,
                &meerkat::ContinuationKey::new("reopen").expect("key"),
            )
            .await
            .expect("status");
        assert!(
            matches!(&applied, meerkat::ContinuationStatus::Applied { session: applied_session, .. } if applied_session == &session)
        );
        assert_eq!(
            fixture
                .delivery
                .inbox()
                .pending_delivery_total()
                .await
                .expect("backlog"),
            0
        );
        assert_eq!(
            continuations
                .submit(&owner, completion("reopen"), 100)
                .await
                .expect("replay"),
            receipt
        );
        assert_eq!(
            continuations
                .continuation_status(
                    &owner,
                    &meerkat::ContinuationKey::new("reopen").expect("key")
                )
                .await
                .expect("status after replay"),
            applied
        );
        runtime.handle().shutdown().await.expect("shutdown");
    }

    /// A candidate launch (composition authority `NonAuthoritative`) arms no
    /// runtime delivery owner. The decision is the delivery's owner state (not
    /// a stopped or failed owner); the pending row stays in the durable inbox
    /// unadmitted over a window although its recipient is live, as a batch
    /// committed to a member mid-turn would at clone time; and the
    /// authoritative boot that follows over the same store applies it once.
    #[tokio::test]
    async fn a_candidate_launch_arms_no_owner_and_leaves_pending_work_for_the_authoritative_boot() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("runtime.sqlite3");
        let session = meerkat_core::SessionId::new();
        let owner = meerkat::ContinuationOwner::Session {
            session_id: session.clone(),
        };
        let key = meerkat::ContinuationKey::new("candidate-held").expect("key");
        {
            let store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("open"));
            let fixture = fixture(dir.path(), store);
            // The recipient is live, so an armed owner would apply this row.
            fixture
                .machine
                .register_session(session.clone())
                .await
                .expect("register recipient");
            let continuations = meerkat::ContinuationOwnerService::new(
                fixture.delivery.inbox(),
                Arc::new(meerkat::SessionAddressResolver),
                Arc::clone(&fixture.machine),
            );
            continuations
                .submit(&owner, completion("candidate-held"), 10)
                .await
                .expect("commit pending completion");
            let spec = fixture.spec.with_composition_authority(
                crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
            );
            let runtime = MobRuntime::bootstrap(spec).await.expect("candidate boot");
            assert!(!runtime.speaks_for_composition());
            // Many passes, had an owner armed.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            assert!(
                fixture.delivery.is_quiesced_for_candidate(),
                "the candidate's arming decision is the delivery's owner state"
            );
            assert!(!fixture.delivery.is_running(), "a candidate arms no owner");
            assert!(
                fixture.delivery.last_pass().is_none(),
                "no pass ran and none is invented"
            );
            assert!(runtime.runtime_delivery_pass().is_none());
            fixture
                .delivery
                .preflight()
                .expect("a candidate holds no delivery ownership");
            assert_eq!(
                fixture
                    .delivery
                    .inbox()
                    .pending_delivery_total()
                    .await
                    .expect("backlog"),
                1,
                "the pending row stays in the durable inbox"
            );
            assert!(matches!(
                continuations
                    .continuation_status(&owner, &key)
                    .await
                    .expect("status"),
                meerkat::ContinuationStatus::Pending { .. }
            ));
            runtime.handle().shutdown().await.expect("shutdown");
        }

        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("reopen"));
        let fixture = fixture(dir.path(), store);
        fixture
            .machine
            .register_session(session.clone())
            .await
            .expect("register recipient");
        let continuations = meerkat::ContinuationOwnerService::new(
            fixture.delivery.inbox(),
            Arc::new(meerkat::SessionAddressResolver),
            Arc::clone(&fixture.machine),
        );
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("authoritative boot");
        let pass = first_pass(&fixture.delivery).await;
        assert_eq!(pass.applied, 1, "{pass:?}");
        assert!(fixture.delivery.is_running());
        assert!(!fixture.delivery.is_quiesced_for_candidate());
        assert!(matches!(
            continuations
                .continuation_status(&owner, &key)
                .await
                .expect("status"),
            meerkat::ContinuationStatus::Applied { .. }
        ));
        runtime.handle().shutdown().await.expect("shutdown");
    }

    /// Race control on the duplicate-owner shape: two launches share one
    /// delivery composition. The authoritative launch arms first; a candidate
    /// whose preflight ran before that arm reaches its own arming step later
    /// and must leave the live owner's observation alone. Then, candidate
    /// first and authoritative later: the later arm changes the shared owner
    /// state, never the earlier launch's own fixed authority.
    #[tokio::test]
    async fn a_delayed_candidate_never_replaces_a_live_owner_and_keeps_its_own_authority() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let service = Arc::clone(&fixture.spec.session_service);
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("authoritative boot");
        first_pass(&fixture.delivery).await;
        // The delayed candidate's arming step, after the live owner armed.
        let decided = fixture
            .delivery
            .arm_for_launch(
                CompositionAuthority::NonAuthoritative,
                Arc::clone(&service),
                Arc::clone(&fixture.machine),
            )
            .expect("a candidate's arming step refuses nothing");
        assert!(decided.is_none(), "a candidate arms nothing");
        assert!(
            fixture.delivery.is_running(),
            "the live owner's observation stands"
        );
        assert!(fixture.delivery.last_pass().is_some());
        assert!(!fixture.delivery.is_quiesced_for_candidate());
        runtime.handle().shutdown().await.expect("shutdown");
        drop(runtime);
        owner_released(&fixture.delivery).await;
        assert!(
            !fixture.delivery.is_quiesced_for_candidate(),
            "a candidate that met a live owner recorded nothing, so the stopped owner never \
             reads quiesced"
        );

        let spec = |label: &str, authority: CompositionAuthority| {
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{label}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
                uuid::Uuid::new_v4()
            ))
            .expect("definition");
            MobBootstrapSpec::new(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                Arc::clone(&service),
            )
            .with_session_runtime_adapter(Arc::clone(&fixture.machine))
            .expect("owner")
            .with_runtime_delivery(fixture.delivery.clone())
            .with_composition_authority(authority)
        };
        let candidate = MobRuntime::bootstrap(spec(
            "candidate-first",
            CompositionAuthority::NonAuthoritative,
        ))
        .await
        .expect("candidate boot");
        assert!(fixture.delivery.is_quiesced_for_candidate());
        let authoritative = MobRuntime::bootstrap(spec(
            "authoritative-later",
            CompositionAuthority::Authoritative,
        ))
        .await
        .expect("authoritative boot after the candidate");
        first_pass(&fixture.delivery).await;
        assert!(fixture.delivery.is_running());
        assert!(!fixture.delivery.is_quiesced_for_candidate());
        assert!(
            !candidate.speaks_for_composition(),
            "the earlier candidate keeps its own fixed authority"
        );
        assert!(authoritative.speaks_for_composition());
        authoritative.handle().shutdown().await.expect("shutdown");
        candidate.handle().shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn restoring_one_mob_never_applies_or_settles_a_foreign_member_row() {
        struct ForeignMemberResolver {
            owner: meerkat::ContinuationOwner,
            address: meerkat_runtime::LogicalRuntimeId,
        }

        #[async_trait::async_trait]
        impl meerkat::ContinuationAddressResolver for ForeignMemberResolver {
            async fn current_address(
                &self,
                owner: &meerkat::ContinuationOwner,
            ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
                if owner == &self.owner {
                    Ok(Some(self.address.clone()))
                } else {
                    Err("unexpected fixture owner".to_string())
                }
            }

            async fn resolve_address(
                &self,
                address: &meerkat_runtime::LogicalRuntimeId,
            ) -> Result<meerkat::AddressResolution, String> {
                if address == &self.address {
                    Ok(meerkat::AddressResolution::NotServed)
                } else {
                    Err("unexpected fixture address".to_string())
                }
            }
        }

        let dir = tempfile::tempdir().expect("temp");
        let fixture = memory_fixture(dir.path());
        let spec = fixture
            .spec
            .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
            .expect("tools");
        let owner = meerkat::ContinuationOwner::Member {
            mob_id: "foreign-mob".to_string(),
            identity: "foreign-member".to_string(),
        };
        let address = meerkat::member_delivery_address(
            "foreign-mob",
            meerkat_core::service::MobIncarnationId::mint(),
            "foreign-member",
            1,
        )
        .expect("foreign address");
        let continuations = meerkat::ContinuationOwnerService::new(
            fixture.delivery.inbox(),
            Arc::new(ForeignMemberResolver {
                owner: owner.clone(),
                address: address.clone(),
            }),
            Arc::clone(&fixture.machine),
        );
        let receipt = continuations
            .submit(&owner, completion("foreign-key"), 1)
            .await
            .expect("commit native foreign continuation");
        let id = meerkat_runtime::RuntimeDeliveryId::new(receipt.delivery_id)
            .expect("native receipt delivery id");
        let runtime = MobRuntime::bootstrap(spec).await.expect("local root boot");
        let pass = first_pass(&fixture.delivery).await;
        assert_eq!(pass.applied, 0, "{pass:?}");
        assert_eq!(pass.refused, 0, "{pass:?}");
        assert_eq!(
            fixture
                .delivery
                .inbox()
                .pending_delivery_total()
                .await
                .expect("backlog"),
            1
        );
        assert!(
            fixture
                .delivery
                .inbox()
                .continuation_admission(&address, &id)
                .await
                .expect("admission")
                .is_none()
        );
        assert!(matches!(
            continuations
                .continuation_status(
                    &owner,
                    &meerkat::ContinuationKey::new("foreign-key").expect("key"),
                )
                .await
                .expect("foreign continuation status"),
            meerkat::ContinuationStatus::Pending { admitted: None, .. }
        ));
        runtime.handle().shutdown().await.expect("shutdown");
    }

    /// The control: a delivery without an issuer projects no consent notice.
    /// After the owner's arming pass, the approved notice a parked consent
    /// owes this store's hosting domain is still uncommitted, and still owed.
    #[tokio::test]
    async fn without_an_issuer_no_consent_notice_is_projected() {
        let dir = tempfile::tempdir().expect("temp");
        let approvals = meerkat_core::ApprovalService::new();
        let fixture = fixture(dir.path(), hosted_store(dir.path()));
        let session = meerkat_core::types::SessionId::new();
        let parked = parked_consent(&approvals, hosting_domain(&fixture.delivery), &session).await;
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("first boot");
        let armed = first_pass(&fixture.delivery).await;
        assert_eq!(armed.owed_projected, 0, "{armed:?}");
        let status = approved_notice_status(
            &fixture.delivery,
            &fixture.machine,
            &session,
            parked.approval_id(),
        )
        .await;
        assert!(
            matches!(status, meerkat::ContinuationStatus::NotCommitted),
            "{status:?}"
        );
        assert_eq!(
            approvals
                .owed_consent_deliveries()
                .expect("owed")
                .owed
                .len(),
            1,
            "still owed"
        );
        runtime.handle().shutdown().await.expect("shutdown");
    }

    /// A delivery composed with the review's issuer projects that issuer's
    /// owed consent notices through its one owner, into its own inbox: the
    /// approved notice a parked consent owes is committed by the owner's
    /// arming pass and stays owed until its admission is recorded. The owner
    /// then stops and releases the inbox as without an issuer.
    #[tokio::test]
    async fn an_issuers_owed_consent_notice_is_projected_into_the_same_inbox() {
        let dir = tempfile::tempdir().expect("temp");
        let approvals = Arc::new(meerkat_core::ApprovalService::new());
        let fixture = fixture_with_issuer(
            dir.path(),
            hosted_store(dir.path()),
            Some(Arc::clone(&approvals)),
        );
        let session = meerkat_core::types::SessionId::new();
        let parked = parked_consent(&approvals, hosting_domain(&fixture.delivery), &session).await;
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("first boot");
        let armed = first_pass(&fixture.delivery).await;
        assert_eq!(
            armed.owed_projected, 1,
            "the owner's own arming round committed the notice: {armed:?}"
        );
        let status = approved_notice_status(
            &fixture.delivery,
            &fixture.machine,
            &session,
            parked.approval_id(),
        )
        .await;
        assert!(
            !matches!(status, meerkat::ContinuationStatus::NotCommitted),
            "the arming pass committed the notice: {status:?}"
        );
        assert_eq!(
            approvals.owed_consent_deliveries().expect("owed").owed,
            vec![(
                parked.approval_id().clone(),
                meerkat_core::ApprovalConsentTransition::Approved
            )],
            "owed until its admission is recorded"
        );
        assert!(fixture.delivery.is_running());
        runtime.handle().shutdown().await.expect("shutdown");
        drop(runtime);
        owner_released(&fixture.delivery).await;
        assert!(!fixture.delivery.is_running());
    }

    /// The projector is composed only with an issuer and a hosting domain: a
    /// delivery without an issuer composes none, and neither does one over a
    /// store without a hosting domain, which scopes no consent.
    #[tokio::test]
    async fn the_projector_needs_an_issuer_and_a_hosting_domain() {
        let dir = tempfile::tempdir().expect("temp");
        let approvals = Arc::new(meerkat_core::ApprovalService::new());
        let hosted = fixture(dir.path(), hosted_store(dir.path()));
        assert!(
            hosted
                .delivery
                .consent_notice_projector(&hosted.machine)
                .is_none(),
            "no issuer, no projector"
        );
        let delivery = hosted
            .delivery
            .with_consent_notice_issuer(Arc::clone(&approvals));
        assert!(
            delivery.consent_notice_projector(&hosted.machine).is_some(),
            "an issuer and a hosting domain"
        );
        let memory = memory_fixture(dir.path());
        assert!(
            memory
                .delivery
                .inbox()
                .hosting_capability()
                .domain_ref()
                .is_none(),
            "an in-memory store has no hosting domain"
        );
        let delivery = memory.delivery.with_consent_notice_issuer(approvals);
        assert!(
            delivery.consent_notice_projector(&memory.machine).is_none(),
            "a store without a hosting domain scopes no consent, so none is composed"
        );
    }

    /// A parked consent whose admitted notice input the runtime cancels
    /// without a run is withdrawn by the owner's own rounds, in the
    /// receipt-first order. The approved notice is admitted into the
    /// registered session as a queued input (no executor serves it), the
    /// delivery owner records its receipt at that admission, and the round
    /// the receipt starts finds the input queued and settles nothing. Only the
    /// runtime's runless-terminal hint can then start the round that reads
    /// the cancellation, withdraws the consent and owes its `Withdrawn`
    /// notice, which the owner then projects, delivers and receipts. The row
    /// never reconciles and never ticks a source.
    #[tokio::test]
    async fn a_cancelled_notice_input_withdraws_its_parked_consent_after_its_receipt() {
        let dir = tempfile::tempdir().expect("temp");
        let approvals = Arc::new(meerkat_core::ApprovalService::new());
        let fixture = fixture_with_issuer(
            dir.path(),
            hosted_store(dir.path()),
            Some(Arc::clone(&approvals)),
        );
        let session = meerkat_core::types::SessionId::new();
        fixture
            .machine
            .register_session(session.clone())
            .await
            .expect("register the destination session");
        let parked = parked_consent(&approvals, hosting_domain(&fixture.delivery), &session).await;
        let approval_id = parked.approval_id().clone();
        let runtime = MobRuntime::bootstrap(fixture.spec)
            .await
            .expect("first boot");
        let mut passes = owner_passes(&fixture.delivery);
        let notice_input = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let meerkat::ContinuationStatus::Applied { input, .. } = approved_notice_status(
                    &fixture.delivery,
                    &fixture.machine,
                    &session,
                    &approval_id,
                )
                .await
                {
                    break input;
                }
                passes.changed().await.expect("owner pass channel open");
            }
        })
        .await
        .expect("the owner admits the approved notice");
        assert_eq!(
            input_terminal(&fixture.machine, &session, &notice_input).await,
            None,
            "no executor serves the session: the notice input stays queued"
        );
        let withdrawn = || {
            approvals
                .get(&approval_id)
                .expect("row")
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason)
        };

        // The delivery owner records the approved notice's receipt at its
        // admission. The round that receipt starts, done once the owner has
        // settled, finds the input queued: it settles nothing.
        let approved_receipt = meerkat_core::ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: notice_input.to_string(),
        };
        tokio::time::timeout(EVENT_GUARD, async {
            while delivery_state(
                &approvals,
                &approval_id,
                meerkat_core::ApprovalConsentTransition::Approved,
            )
            .as_ref()
                != Some(&approved_receipt)
            {
                passes.changed().await.expect("owner pass channel open");
            }
        })
        .await
        .expect("the owner records the approved notice's receipt");
        owner_settled(&mut passes).await;
        assert_eq!(withdrawn(), None, "a queued input proves nothing");

        assert!(
            fixture
                .machine
                .cancel_input_if_present(&session, &notice_input, "the notice was withdrawn")
                .await
                .expect("cancel the notice input"),
            "the queued notice input is observed"
        );
        assert_eq!(
            input_terminal(&fixture.machine, &session, &notice_input).await,
            Some(
                meerkat_runtime::input_state::InputTerminalOutcome::Abandoned {
                    reason: meerkat_runtime::input_state::InputAbandonReason::Cancelled,
                }
            ),
            "the runtime abandoned the queued notice input"
        );
        let withdrawn_input = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let meerkat::ContinuationStatus::Applied { input, .. } = notice_status(
                    &fixture.delivery,
                    &fixture.machine,
                    &session,
                    &approval_id,
                    meerkat_core::ApprovalConsentTransition::Withdrawn,
                )
                .await
                {
                    break input;
                }
                passes.changed().await.expect("owner pass channel open");
            }
        })
        .await
        .expect("the runtime's runless-terminal hint starts the round that withdraws the consent");
        let withdrawn_receipt = meerkat_core::ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: withdrawn_input.to_string(),
        };
        tokio::time::timeout(EVENT_GUARD, async {
            while delivery_state(
                &approvals,
                &approval_id,
                meerkat_core::ApprovalConsentTransition::Withdrawn,
            )
            .as_ref()
                != Some(&withdrawn_receipt)
            {
                passes.changed().await.expect("owner pass channel open");
            }
        })
        .await
        .expect("the owner records the Withdrawn notice's receipt");

        assert_eq!(
            withdrawn(),
            Some(
                meerkat_core::approval::ApprovalConsentWithdrawalReason::NoticeInputEnded {
                    input_id: notice_input.to_string(),
                }
            )
        );
        let deliveries: Vec<_> = approvals
            .get(&approval_id)
            .expect("row")
            .consent
            .expect("consent")
            .deliveries
            .into_iter()
            .map(|delivery| (delivery.transition, delivery.state))
            .collect();
        assert_eq!(
            deliveries,
            vec![
                (
                    meerkat_core::ApprovalConsentTransition::Approved,
                    approved_receipt,
                ),
                (
                    meerkat_core::ApprovalConsentTransition::Withdrawn,
                    withdrawn_receipt,
                ),
            ],
            "both notices are receipted with the inputs they were admitted as"
        );
        runtime.handle().shutdown().await.expect("shutdown");
    }
}
