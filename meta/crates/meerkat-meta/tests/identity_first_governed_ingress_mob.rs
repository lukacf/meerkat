//! A trusted governed input sent through the identity runtime reaches a real
//! mob member whose session runs on the real scoped grant composition. The
//! host binds it with the member session actor's actual controller, and its
//! settlement custody follows the native admission's own answer: settled once
//! from the canonical acknowledgement, withdrawn on a definite refusal, and
//! never lost to an observation deadline or a departed caller.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "support/governed_mob.rs"]
mod governed_support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use governed_support::*;
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentRuntimeId, BoundGovernedInput, DurabilityPolicy, DurableAgentSpec,
    GovernedAdmission, GovernedBindContext, GovernedDeliveryCustody, GovernedEntryFence,
    GovernedEntryOutcome, GovernedEntryPermit, GovernedEntryRefusal, GovernedInputBinder,
    GovernedInputError, GovernedSettlement, IdentityRuntime, IdentityRuntimeConfig,
    IdentityRuntimeError, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
    NativeInputAdmissionSettlement, SessionBridge, TurnOutcome, TurnOutput, TurnTracking,
};

/// The host's settlement custody: it records its one settlement, and a drop
/// without one is the host's withdrawal.
#[derive(Default)]
struct Custody {
    admitted: Mutex<Vec<GovernedAdmission>>,
    withdrawn: AtomicUsize,
    /// Custody the runtime handed back unresolved; the host holds it.
    retained: Mutex<Vec<Box<dyn GovernedSettlement>>>,
}

struct Settlement {
    custody: Arc<Custody>,
    settled: AtomicBool,
}

impl GovernedSettlement for Settlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        self.settled.store(true, Ordering::SeqCst);
        self.custody
            .admitted
            .lock()
            .expect("custody")
            .push(admission.clone());
        Ok(())
    }
}

impl Drop for Settlement {
    fn drop(&mut self) {
        if !self.settled.load(Ordering::SeqCst) {
            self.custody.withdrawn.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[derive(Clone, Copy)]
enum Binding {
    /// The authenticated original for the fixture's bound destination.
    Original,
    /// An original the adapter authenticated for another destination
    /// generation: native admission refuses it.
    WrongDestination,
    /// The same original event again under a fresh transport
    /// authentication: native admission replays it.
    Retry,
    /// The raw input handed back unbound.
    Unbound,
}

/// The trusted host's binder: it binds for the session it is handed, with
/// the controller it is handed, and pairs the input with its custody. A
/// binder whose input was authored from a registered row rechecks it against
/// the row it is handed, refusing (and withdrawing its custody) on any
/// difference.
struct FixtureBinder {
    fixture: Arc<Fixture>,
    binding: Binding,
    custody: Arc<Custody>,
    contexts: Mutex<Vec<(SessionId, ControllerModelSelection)>>,
    bound_input: Mutex<Option<meerkat_core::lifecycle::InputId>>,
    /// Every registered row a bind was handed.
    rows: Mutex<Vec<DurableAgentSpec>>,
    /// The registered row the input was authored from.
    authored_from: Option<DurableAgentSpec>,
    /// Holds the bind (entered, released) after it is handed the row.
    hold: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    /// The host's final-entry fence the bound input carries, if any.
    fence: Option<Arc<dyn GovernedEntryFence>>,
}

impl FixtureBinder {
    fn new(fixture: &Arc<Fixture>, binding: Binding) -> Arc<Self> {
        Arc::new(Self::build(fixture, binding, None, None))
    }

    fn authored_from(fixture: &Arc<Fixture>, row: DurableAgentSpec) -> Arc<Self> {
        Arc::new(Self::build(fixture, Binding::Original, Some(row), None))
    }

    fn held(
        fixture: &Arc<Fixture>,
        row: DurableAgentSpec,
        entered: &Arc<tokio::sync::Notify>,
        release: &Arc<tokio::sync::Notify>,
    ) -> Arc<Self> {
        Arc::new(Self::build(
            fixture,
            Binding::Original,
            Some(row),
            Some((Arc::clone(entered), Arc::clone(release))),
        ))
    }

    fn build(
        fixture: &Arc<Fixture>,
        binding: Binding,
        authored_from: Option<DurableAgentSpec>,
        hold: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    ) -> Self {
        Self {
            fixture: Arc::clone(fixture),
            binding,
            custody: Arc::new(Custody::default()),
            contexts: Mutex::new(Vec::new()),
            bound_input: Mutex::new(None),
            rows: Mutex::new(Vec::new()),
            authored_from,
            hold,
            fence: None,
        }
    }

    fn rows(&self) -> Vec<DurableAgentSpec> {
        self.rows.lock().expect("rows").clone()
    }

    fn admitted(&self) -> Vec<GovernedAdmission> {
        self.custody.admitted.lock().expect("custody").clone()
    }

    fn withdrawn(&self) -> usize {
        self.custody.withdrawn.load(Ordering::SeqCst)
    }

    fn retained(&self) -> usize {
        self.custody.retained.lock().expect("custody").len()
    }
}

#[async_trait]
impl GovernedInputBinder for FixtureBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        self.contexts.lock().expect("contexts").push((
            context.session_id.clone(),
            context.controller.selection().clone(),
        ));
        self.rows.lock().expect("rows").push(context.spec.clone());
        if let Some((entered, release)) = &self.hold {
            entered.notify_one();
            release.notified().await;
        }
        if self
            .authored_from
            .as_ref()
            .is_some_and(|row| *row != context.spec)
        {
            // The host's custody for this delivery, withdrawn with the
            // refusal: nothing is submitted.
            drop(Settlement {
                custody: Arc::clone(&self.custody),
                settled: AtomicBool::new(false),
            });
            return Err("the input was authored from another registered row".to_string());
        }
        let bound = match self.binding {
            Binding::Original => {
                self.fixture
                    .bound_with(&context.controller, |_| {}, "original-authentication")
            }
            Binding::WrongDestination => self.fixture.bound_with(
                &context.controller,
                |candidate| candidate.target.context_generation += 1,
                "original-authentication",
            ),
            Binding::Retry => self.fixture.bound_with(
                &context.controller,
                |_| {},
                "retry-transport-authentication",
            ),
            Binding::Unbound => input,
        };
        *self.bound_input.lock().expect("bound input") = Some(bound.id().clone());
        Ok(BoundGovernedInput {
            input: bound,
            settlement: Box::new(Settlement {
                custody: Arc::clone(&self.custody),
                settled: AtomicBool::new(false),
            }),
            entry_fence: self.fence.clone(),
        })
    }

    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _detail: &str) {
        self.custody
            .retained
            .lock()
            .expect("custody")
            .push(settlement);
    }
}

/// How the answer to a governed submission is lost after the real native
/// admission ran.
#[derive(Clone, Copy)]
enum LostAnswer {
    /// The bridge could not tell whether native admission took the input.
    Unresolved,
    /// Native work failed with an internal error after admission.
    PostAdmissionInternal,
    /// The submission and its native custody were lost before native
    /// admission settled it.
    CustodyLost,
}

mod losing {
    use std::sync::Arc;

    use async_trait::async_trait;
    use meerkat_core::types::HandlingMode;
    use meerkat_meta::actor_loop_health::ActorLoopHealth;
    use meerkat_meta::identity_first::*;
    use meerkat_mob::MobError;
    use meerkat_mob::ids::AgentIdentity as MobAgentIdentity;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::LostAnswer;

    /// The real mob session bridge, except that after the real governed
    /// delivery ran its answer is replaced by an unresolved one.
    pub struct LosingBridge {
        pub inner: MobSessionBridge,
        pub lose: LostAnswer,
        /// Real native acknowledgements of new work seen before an answer
        /// was replaced.
        pub native_accepted: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SessionBridge for LosingBridge {
        async fn begin_governed_delivery(
            &self,
            runtime_id: &AgentRuntimeId,
            session_id: &meerkat_core::types::SessionId,
            input: meerkat_runtime::Input,
            custody: GovernedDeliveryCustody,
        ) -> Result<GovernedDelivery, BridgeAdmissionError> {
            if matches!(self.lose, LostAnswer::CustodyLost) {
                // Taken past its guard and lost unsettled: no native
                // settlement is ever observed.
                drop((input, custody.submit()));
                return Err(BridgeAdmissionError::GovernedInput(
                    GovernedInputError::Mob(Box::new(MobError::Internal(
                        "the submission was lost".to_string(),
                    ))),
                ));
            }
            // The real native admission runs, settles the custody and must
            // actually accept the input as new work; only then is its answer
            // replaced, so a native refusal can never pass for a lost answer.
            let real = self
                .inner
                .begin_governed_delivery(runtime_id, session_id, input, custody)
                .await;
            if matches!(
                &real,
                Ok(GovernedDelivery { admission: Some(admission), .. })
                    if !admission.deduplicated
            ) {
                self.native_accepted.fetch_add(1, Ordering::SeqCst);
            }
            Err(BridgeAdmissionError::GovernedInput(match self.lose {
                LostAnswer::Unresolved => GovernedInputError::AdmissionUnresolved {
                    detail: "the admission answer was lost".to_string(),
                },
                LostAnswer::PostAdmissionInternal | LostAnswer::CustodyLost => {
                    GovernedInputError::Mob(Box::new(MobError::Internal(
                        "a stage failed after admission".to_string(),
                    )))
                }
            }))
        }

        fn observe_actor_loop_health(&self, health: Arc<ActorLoopHealth>) {
            self.inner.observe_actor_loop_health(health);
        }

        async fn reload_member_registration(
            &self,
            runtime_id: &AgentRuntimeId,
        ) -> Result<Option<BridgeMemberReload>, BridgeError> {
            self.inner.reload_member_registration(runtime_id).await
        }

        async fn release_member_run_starts_for_published_tools(
            &self,
            member_id: &MobAgentIdentity,
        ) -> Result<(), BridgeError> {
            self.inner
                .release_member_run_starts_for_published_tools(member_id)
                .await
        }

        async fn member_durability(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Option<MemberDurability> {
            self.inner.member_durability(session_id).await
        }

        async fn raw_member_alias_exists(&self, alias: &str) -> Result<bool, BridgeError> {
            self.inner.raw_member_alias_exists(alias).await
        }

        async fn recover_committed_boundary(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<CommittedBoundaryRepair, BridgeError> {
            self.inner.recover_committed_boundary(session_id).await
        }

        async fn reset_member_to_successor(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            self.inner
                .reset_member_to_successor(identity, spec, draft)
                .await
        }

        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            self.inner.create_session(identity, spec, draft).await
        }

        async fn resume_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            draft: &AgentBuildDraft,
            session_id: &meerkat_core::types::SessionId,
            resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            self.inner
                .resume_session(identity, spec, draft, session_id, resume_from_role)
                .await
        }

        async fn deliver_admitted(
            &self,
            runtime_id: &AgentRuntimeId,
            delivery: BridgeDelivery,
        ) -> Result<meerkat_core::types::SessionId, BridgeError> {
            self.inner.deliver_admitted(runtime_id, delivery).await
        }

        async fn deliver_host_human_input(
            &self,
            runtime_id: &AgentRuntimeId,
            expected_session: &meerkat_core::SessionId,
            delivery: BridgeDelivery,
        ) -> Result<meerkat_core::SessionId, BridgeError> {
            self.inner
                .deliver_host_human_input(runtime_id, expected_session, delivery)
                .await
        }

        async fn begin_awaiting_commit(
            &self,
            runtime_id: &AgentRuntimeId,
            content: &meerkat_core::ContentInput,
            system_prompt: Option<&str>,
            injected_context: &[meerkat_core::ContentInput],
            handling_mode: HandlingMode,
            interaction_id: Option<&str>,
        ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
            self.inner
                .begin_awaiting_commit(
                    runtime_id,
                    content,
                    system_prompt,
                    injected_context,
                    handling_mode,
                    interaction_id,
                )
                .await
        }

        fn tracks_turn_output(&self) -> bool {
            self.inner.tracks_turn_output()
        }

        async fn governed_controller(
            &self,
            runtime_id: &AgentRuntimeId,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
            self.inner.governed_controller(runtime_id, session_id).await
        }

        async fn begin_delivery_with_output(
            &self,
            runtime_id: &AgentRuntimeId,
            delivery: BridgeDelivery,
        ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
            self.inner
                .begin_delivery_with_output(runtime_id, delivery)
                .await
        }

        async fn checkpoint_session(
            &self,
            runtime_id: &AgentRuntimeId,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            self.inner.checkpoint_session(runtime_id, session_id).await
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.inner.retire_member(runtime_id).await
        }

        async fn retire_reset_superseded_member(
            &self,
            runtime_id: &AgentRuntimeId,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), BridgeError> {
            self.inner
                .retire_reset_superseded_member(runtime_id, session_id)
                .await
        }

        async fn wire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.inner.wire_peer(a, b).await
        }

        async fn wire_peers_batch(
            &self,
            edges: &[(AgentRuntimeId, AgentRuntimeId)],
        ) -> Result<(), BridgeError> {
            self.inner.wire_peers_batch(edges).await
        }

        async fn current_member_wires(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            self.inner.current_member_wires().await
        }

        async fn current_member_wires_any_half(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            self.inner.current_member_wires_any_half().await
        }

        async fn unwire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.inner.unwire_peer(a, b).await
        }

        async fn inspect_member(
            &self,
            runtime_id: &AgentRuntimeId,
        ) -> Result<MemberInspection, BridgeError> {
            self.inner.inspect_member(runtime_id).await
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            self.inner.live_member(identity).await
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.inner.live_member_scope(identity).await
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            self.inner.ensure_member_live(identity).await
        }

        async fn identity_convergence_status(
            &self,
            identity: &AgentIdentity,
        ) -> Result<
            meerkat_mob::identity::IdentityStoredObservation<
                meerkat_mob::identity::IdentityConvergenceStatus,
            >,
            BridgeError,
        > {
            self.inner.identity_convergence_status(identity).await
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.inner.member_binding_history().await
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.inner.last_member_binding(identity).await
        }

        async fn identity_management(
            &self,
            identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            self.inner.identity_management(identity).await
        }

        async fn durable_identity_members(
            &self,
        ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
            self.inner.durable_identity_members().await
        }

        async fn durable_session_held(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<bool, BridgeError> {
            self.inner.durable_session_held(session_id).await
        }

        async fn register_session_runtime_state(
            &self,
            session_id: &meerkat_core::types::SessionId,
            identity: &AgentIdentity,
            generation: ContinuityGeneration,
            checkpoint_version: CheckpointVersion,
            fencing_token: FencingToken,
        ) -> Result<CheckpointVersion, BridgeError> {
            self.inner
                .register_session_runtime_state(
                    session_id,
                    identity,
                    generation,
                    checkpoint_version,
                    fencing_token,
                )
                .await
        }

        async fn suspend_session_runtime_state(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), BridgeError> {
            self.inner.suspend_session_runtime_state(session_id).await
        }

        async fn unregister_session_runtime_state(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), BridgeError> {
            self.inner
                .unregister_session_runtime_state(session_id)
                .await
        }
    }
}

async fn governed_runtime(
    fixture: &Arc<Fixture>,
    admission_budget: Option<Duration>,
) -> (
    Arc<IdentityRuntime>,
    meerkat_meta::identity_first::AgentIdentity,
) {
    governed_runtime_losing(fixture, admission_budget, None).await
}

async fn governed_runtime_losing(
    fixture: &Arc<Fixture>,
    admission_budget: Option<Duration>,
    lose: Option<(LostAnswer, Arc<AtomicUsize>)>,
) -> (
    Arc<IdentityRuntime>,
    meerkat_meta::identity_first::AgentIdentity,
) {
    governed_runtime_composed(
        fixture,
        admission_budget,
        lose,
        AgentAddressability::Addressable,
    )
    .await
}

async fn governed_runtime_composed(
    fixture: &Arc<Fixture>,
    admission_budget: Option<Duration>,
    lose: Option<(LostAnswer, Arc<AtomicUsize>)>,
    addressability: AgentAddressability,
) -> (
    Arc<IdentityRuntime>,
    meerkat_meta::identity_first::AgentIdentity,
) {
    let mut bridge = MobSessionBridge::new(fixture.handle.clone())
        .with_controller_pin_source(fixture.service.clone());
    if let Some(budget) = admission_budget {
        bridge = bridge.with_actor_admission_budget(budget);
    }
    let bridge: Arc<dyn SessionBridge> = match lose {
        Some((lose, native_accepted)) => Arc::new(losing::LosingBridge {
            inner: bridge,
            lose,
            native_accepted,
        }),
        None => Arc::new(bridge),
    };
    let store: Arc<dyn ContinuityStore> =
        Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "governed-ingress-mob".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }));
    // Only the spec: the fixture's mob already holds the member live on its
    // session, and Core's member history is the identity's binding.
    let identity = meerkat_meta::identity_first::AgentIdentity::parse(MEMBER).unwrap();
    runtime
        .register(DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        })
        .await;
    (runtime, identity)
}

fn raw_prompt() -> Input {
    Input::Prompt(PromptInput::new(PROMPT, None))
}

fn completed_with_answer(outcome: &TurnOutcome) -> bool {
    matches!(
        outcome,
        TurnOutcome::Completed {
            output: TurnOutput::Text { text, .. }
        } if text == ANSWER
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_send_is_admitted_settled_once_and_completes_under_its_ticket() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    // The host authored the input from the identity's complete registered
    // row, which still matches at bind.
    let registered = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");
    let binder = FixtureBinder::authored_from(&fixture, registered.spec.clone());

    let sent = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect("the governed send is admitted");

    // The binder was handed exactly the member's session, its actor's
    // actual controller and the complete registered row.
    assert_eq!(
        binder.contexts.lock().expect("contexts").as_slice(),
        [(fixture.session_id.clone(), selection())]
    );
    assert_eq!(binder.rows(), vec![registered.spec]);
    // Settled exactly once, from the native acknowledgement of this exact
    // bound input; nothing withdrawn.
    let bound_input = binder
        .bound_input
        .lock()
        .expect("bound input")
        .clone()
        .expect("bound");
    assert_eq!(sent.admission.input_id, bound_input);
    assert!(!sent.admission.deduplicated);
    assert_eq!(binder.admitted(), vec![sent.admission.clone()]);
    assert_eq!(binder.withdrawn(), 0);
    assert!(sent.settlement.is_ok());

    let TurnTracking::Tracked(ticket) = sent.ticketed.turn else {
        panic!("a governed send is tracked by its own ticket");
    };
    let outcome = runtime.wait_for_turn(&identity, ticket, WAIT).await;
    assert!(completed_with_answer(&outcome), "{outcome:?}");
    let requests = fixture.provider.requests();
    assert_eq!(requests.len(), 1, "one native controller request");
    assert!(mentions_prompt(&requests[0]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_or_unsubmitted_governed_input_withdraws_its_custody() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;

    // Native admission refuses an original for another destination and
    // settles its custody `NotAdmitted`: the host custody is withdrawn,
    // never settled.
    let refused = FixtureBinder::new(&fixture, Binding::WrongDestination);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), refused.clone())
        .await
        .expect_err("native admission refuses the other destination");
    assert!(
        matches!(
            &error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(mob))
                if matches!(**mob, MobError::MemberInputRefused { .. })
        ),
        "{error}"
    );
    assert!(refused.admitted().is_empty());
    assert_eq!(refused.withdrawn(), 1);

    // An input handed back unbound is never submitted: withdrawn too.
    let unbound = FixtureBinder::new(&fixture, Binding::Unbound);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), unbound.clone())
        .await
        .expect_err("an unbound input is refused before submission");
    assert!(
        matches!(
            error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Unbound)
        ),
        "{error}"
    );
    assert!(unbound.admitted().is_empty());
    assert_eq!(unbound.withdrawn(), 1);

    assert!(
        fixture.provider.requests().is_empty(),
        "no model call for a refused or unsubmitted input"
    );
}

/// A contributor whose admission its source owner has not settled yet
/// (retained custody `AdmissionPending`) is not admissible yet: native
/// admission answers the governed send with the typed
/// `OriginalAdmissionPending` readiness failure and settles it
/// `NotAdmitted`, so the host custody is withdrawn and no model is called.
/// Once the owner retains the contributor, the same send is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_contributor_refuses_the_governed_send_until_it_is_retained() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let registered = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");

    fixture
        .adapter
        .sources_pending
        .store(true, Ordering::SeqCst);
    let pending = FixtureBinder::authored_from(&fixture, registered.spec.clone());
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), pending.clone())
        .await
        .expect_err("a contributor whose admission is pending is not admissible yet");
    assert!(
        matches!(
            &error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(mob))
                if matches!(
                    &**mob,
                    MobError::MemberInputRefused {
                        error: meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                            reason: meerkat_core::authorization::ControllerReadinessFailure::OriginalAdmissionPending,
                        },
                        ..
                    }
                )
        ),
        "{error}"
    );
    assert!(pending.admitted().is_empty());
    assert_eq!(pending.withdrawn(), 1);
    assert!(
        fixture.provider.requests().is_empty(),
        "no model call while a contributor's admission is pending"
    );

    fixture
        .adapter
        .sources_pending
        .store(false, Ordering::SeqCst);
    let retained = FixtureBinder::authored_from(&fixture, registered.spec);
    let sent = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), retained.clone())
        .await
        .expect("the send is admitted once the contributors are retained");
    assert_eq!(retained.admitted(), vec![sent.admission]);
    assert_eq!(retained.withdrawn(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_departed_caller_and_an_elapsed_deadline_never_lose_an_entered_admission() {
    let fixture = Arc::new(Fixture::new().await);
    // An observation budget far shorter than the held native admission.
    let budget = Duration::from_millis(50);
    let (runtime, identity) = governed_runtime(&fixture, Some(budget)).await;

    // Hold the actual native admission at its original-custody read, past the
    // caller-liveness check.
    fixture.adapter.gate.hold();
    let binder = FixtureBinder::new(&fixture, Binding::Original);
    let caller = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        let binder = binder.clone();
        async move {
            runtime
                .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder)
                .await
        }
    });
    wait_until("the native admission is entered", || {
        fixture.adapter.gate.entered() > 0
    })
    .await;
    // Well past the budget: neither settled nor withdrawn while the native
    // answer is pending.
    tokio::time::sleep(budget * 6).await;
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 0);
    // The caller goes away; the runtime-owned operation keeps the submission.
    caller.abort();
    assert!(caller.await.expect_err("aborted").is_cancelled());
    fixture.adapter.gate.release();

    // The actual native acknowledgement settles the custody exactly once.
    wait_until("the admission settles", || !binder.admitted().is_empty()).await;
    let admitted = binder.admitted();
    assert_eq!(admitted.len(), 1);
    assert_eq!(
        Some(admitted[0].input_id.clone()),
        binder.bound_input.lock().expect("bound input").clone()
    );
    assert_eq!(binder.withdrawn(), 0);
    // And the admitted turn ran its one native controller request.
    wait_until("the admitted turn runs", || {
        fixture.provider.requests().len() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_replay_reports_the_canonical_input_and_withdraws_the_retry_custody() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let original = FixtureBinder::new(&fixture, Binding::Original);
    let first = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), original.clone())
        .await
        .expect("the original is admitted");
    let TurnTracking::Tracked(ticket) = first.ticketed.turn else {
        panic!("tracked");
    };
    let _ = runtime.wait_for_turn(&identity, ticket, WAIT).await;
    assert_eq!(original.admitted().len(), 1);

    // The same original event under a fresh transport authentication is
    // replayed natively onto the canonical input: it never promotes, its own
    // custody is withdrawn, and the original's settlement is untouched.
    let retry = FixtureBinder::new(&fixture, Binding::Retry);
    let replayed = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), retry.clone())
        .await
        .expect("the retry is answered by native replay");
    assert!(replayed.admission.deduplicated);
    assert_eq!(replayed.admission.input_id, first.admission.input_id);
    assert!(retry.admitted().is_empty());
    assert_eq!(retry.withdrawn(), 1);
    assert_eq!(original.admitted().len(), 1);
    assert_eq!(original.withdrawn(), 0);
    assert_eq!(
        fixture.provider.requests().len(),
        1,
        "a replayed original runs no second turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolved_answer_after_the_caller_left_keeps_custody_with_the_host() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, identity) = governed_runtime_losing(
        &fixture,
        None,
        Some((LostAnswer::Unresolved, Arc::clone(&native_accepted))),
    )
    .await;
    fixture.adapter.gate.hold();
    let binder = FixtureBinder::new(&fixture, Binding::Original);
    let caller = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        let binder = binder.clone();
        async move {
            runtime
                .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder)
                .await
        }
    });
    wait_until("the native admission is entered", || {
        fixture.adapter.gate.entered() > 0
    })
    .await;
    // The caller is gone before the unresolved answer arrives.
    caller.abort();
    assert!(caller.await.expect_err("aborted").is_cancelled());
    fixture.adapter.gate.release();
    // With no caller to receive it, the runtime-owned operation still hands
    // the custody to the host: never promoted, never withdrawn.
    wait_until("the unresolved custody is retained", || {
        binder.retained() == 1
    })
    .await;
    // The replaced answer followed a real native acceptance of new work.
    assert_eq!(native_accepted.load(Ordering::SeqCst), 1);
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_post_admission_internal_error_is_unresolved_not_a_refusal() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, identity) = governed_runtime_losing(
        &fixture,
        None,
        Some((
            LostAnswer::PostAdmissionInternal,
            Arc::clone(&native_accepted),
        )),
    )
    .await;
    let binder = FixtureBinder::new(&fixture, Binding::Original);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("the answer is unresolved");
    // The replaced answer followed a real native acceptance of new work.
    assert_eq!(native_accepted.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved { .. })
        ),
        "{error}"
    );
    assert_eq!(binder.retained(), 1);
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_native_custody_keeps_the_host_custody_with_its_owner() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, identity) = governed_runtime_losing(
        &fixture,
        None,
        Some((LostAnswer::CustodyLost, Arc::clone(&native_accepted))),
    )
    .await;
    let binder = FixtureBinder::new(&fixture, Binding::Original);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("no native settlement was observed");
    assert_eq!(native_accepted.load(Ordering::SeqCst), 0);
    // An error alone, of any kind, never withdraws: without a `NotAdmitted`
    // settlement the custody stays with the host.
    assert!(
        matches!(
            error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved { .. })
        ),
        "{error}"
    );
    assert_eq!(binder.retained(), 1);
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uncertain_native_admission_keeps_the_host_custody_with_its_owner() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    // The real native admission fails inside its admission attempt (the
    // third custody read), so it settles its custody `Uncertain`.
    fixture
        .adapter
        .panic_at_custody_read
        .store(3, Ordering::SeqCst);
    let binder = FixtureBinder::new(&fixture, Binding::Original);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("the admission attempt failed");
    assert_eq!(
        fixture.adapter.gate.entered(),
        3,
        "the failure was inside the admission attempt"
    );
    assert!(
        matches!(
            error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved { .. })
        ),
        "{error}"
    );
    assert_eq!(binder.retained(), 1);
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 0);
}

/// A native custody that records every settlement it receives.
struct CountingCustody(Arc<Mutex<Vec<NativeInputAdmissionSettlement>>>);

impl meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody for CountingCustody {
    fn settle(
        self: Box<Self>,
        settlement: NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        self.0.lock().expect("settlements").push(settlement);
        None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_delivery_dropped_before_its_first_poll_settles_not_admitted_once() {
    let fixture = Arc::new(Fixture::new().await);
    let bridge: Arc<dyn SessionBridge> = Arc::new(
        MobSessionBridge::new(fixture.handle.clone())
            .with_controller_pin_source(fixture.service.clone()),
    );
    let runtime_id = AgentRuntimeId::parse(MEMBER).expect("runtime id");
    let settled = Arc::new(Mutex::new(Vec::new()));
    // The public bridge future is created and dropped without ever being
    // polled: its guarded custody still settles, exactly once.
    let delivery = bridge.begin_governed_delivery(
        &runtime_id,
        &fixture.session_id,
        fixture.bound_with(&fixture.pin, |_| {}, "original-authentication"),
        GovernedDeliveryCustody::new(Box::new(CountingCustody(Arc::clone(&settled)))),
    );
    drop(delivery);
    assert_eq!(
        *settled.lock().expect("settlements"),
        vec![NativeInputAdmissionSettlement::NotAdmitted]
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        settled.lock().expect("settlements").len(),
        1,
        "settled exactly once"
    );
    assert_eq!(fixture.adapter.gate.entered(), 0, "nothing was submitted");
    assert!(fixture.provider.requests().is_empty());
}

/// The identity's registered row with a same-profile instructions change.
fn with_instructions(row: &DurableAgentSpec, instructions: &str) -> DurableAgentSpec {
    let mut changed = row.clone();
    changed.additional_instructions = vec![instructions.to_string()];
    changed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_input_authored_from_a_superseded_row_is_refused_at_bind_and_submits_nothing() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let row_a = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row")
        .spec;
    // A same-profile instructions change commits: same profile, session and
    // lease, a different registered row.
    let row_b = with_instructions(&row_a, "Answer in one sentence.");
    runtime
        .update_spec(row_b.clone())
        .await
        .expect("the row change commits");

    let binder = FixtureBinder::authored_from(&fixture, row_a);
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("the stale authored input is refused");
    assert!(
        matches!(
            &error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Bind(_))
        ),
        "{error}"
    );
    assert_eq!(binder.rows(), vec![row_b], "bind is handed the current row");
    assert!(binder.admitted().is_empty());
    assert_eq!(binder.withdrawn(), 1);
    assert!(
        binder.bound_input.lock().expect("bound input").is_none(),
        "nothing was bound"
    );
    assert_eq!(
        fixture.adapter.gate.entered(),
        0,
        "the native admission was never entered"
    );
    assert!(fixture.provider.requests().is_empty(), "no model call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_change_waits_behind_an_admission_checked_against_the_old_row() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let row_a = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row")
        .spec;
    let row_b = with_instructions(&row_a, "Answer in one sentence.");

    // The bind for an input authored from row A is held after it was handed
    // the row, inside the delivery's lifecycle custody.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let binder = FixtureBinder::held(&fixture, row_a.clone(), &entered, &release);
    let send = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        let binder = binder.clone();
        async move {
            runtime
                .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder)
                .await
        }
    });
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .expect("the bind is entered");

    // The real writer is polled once and parks on the identity's lifecycle
    // lock: it has not committed row B while the admission is in flight.
    let mut writer = std::pin::pin!(runtime.update_spec(row_b.clone()));
    assert!(
        std::future::Future::poll(
            writer.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        )
        .is_pending(),
        "the row change waits behind admission"
    );
    assert_eq!(
        runtime
            .registered_spec(&identity)
            .await
            .expect("registered row")
            .spec,
        row_a,
        "row A is still the registered row"
    );

    // The queued change does not refuse the input: row A is valid at bind
    // and the input is admitted and settled once.
    release.notify_one();
    let sent = tokio::time::timeout(WAIT, send)
        .await
        .expect("the send completes")
        .expect("the send task does not panic")
        .expect("row A is admitted");
    assert_eq!(binder.rows(), vec![row_a]);
    assert_eq!(binder.admitted(), vec![sent.admission.clone()]);
    assert_eq!(binder.withdrawn(), 0);

    // Then the same writer future commits row B.
    tokio::time::timeout(WAIT, writer)
        .await
        .expect("the row change completes")
        .expect("row B commits");
    assert_eq!(
        runtime
            .registered_spec(&identity)
            .await
            .expect("registered row")
            .spec,
        row_b
    );
    let TurnTracking::Tracked(ticket) = sent.ticketed.turn else {
        panic!("a governed send is tracked by its own ticket");
    };
    let outcome = runtime.wait_for_turn(&identity, ticket, WAIT).await;
    assert!(completed_with_answer(&outcome), "{outcome:?}");
}

/// A host's retained entry owner (for example one Console login): it admits
/// uses while open, refuses with its typed cause once closed, and a close
/// waits for the uses already inside it, collecting their outcomes.
#[derive(Default)]
struct TestFence {
    state: Mutex<TestFenceState>,
}

#[derive(Default)]
struct TestFenceState {
    closed: Option<GovernedEntryRefusal>,
    inside: usize,
    entered: usize,
    outcomes: Vec<GovernedEntryOutcome>,
}

struct TestPermit {
    fence: Arc<TestFence>,
    finished: bool,
}

impl GovernedEntryPermit for TestPermit {
    fn finish(mut self: Box<Self>, outcome: GovernedEntryOutcome) {
        self.finished = true;
        let mut state = self.fence.state.lock().expect("fence");
        state.inside -= 1;
        state.outcomes.push(outcome);
    }
}

impl Drop for TestPermit {
    fn drop(&mut self) {
        if !self.finished {
            let mut state = self.fence.state.lock().expect("fence");
            state.inside -= 1;
            state.outcomes.push(GovernedEntryOutcome::Uncertain);
        }
    }
}

/// The fence is entered through an `Arc` handle so its permits can report
/// back to it.
struct TestFenceHandle(Arc<TestFence>);

impl GovernedEntryFence for TestFenceHandle {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        let mut state = self.0.state.lock().expect("fence");
        if let Some(refusal) = &state.closed {
            return Err(refusal.clone());
        }
        state.inside += 1;
        state.entered += 1;
        Ok(Box::new(TestPermit {
            fence: Arc::clone(&self.0),
            finished: false,
        }))
    }
}

impl TestFence {
    fn new() -> (Arc<Self>, Arc<dyn GovernedEntryFence>) {
        let fence = Arc::new(Self::default());
        let handle: Arc<dyn GovernedEntryFence> = Arc::new(TestFenceHandle(Arc::clone(&fence)));
        (fence, handle)
    }

    /// Close the fence (new entries refuse with `refusal`), then wait for the
    /// uses already inside; returns every reported outcome.
    async fn close(&self, refusal: GovernedEntryRefusal) -> Vec<GovernedEntryOutcome> {
        self.state.lock().expect("fence").closed = Some(refusal);
        loop {
            {
                let state = self.state.lock().expect("fence");
                if state.inside == 0 {
                    return state.outcomes.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn entered(&self) -> usize {
        self.state.lock().expect("fence").entered
    }

    fn outcomes(&self) -> Vec<GovernedEntryOutcome> {
        self.state.lock().expect("fence").outcomes.clone()
    }
}

fn fenced_binder(
    fixture: &Arc<Fixture>,
    binding: Binding,
    handle: &Arc<dyn GovernedEntryFence>,
) -> Arc<FixtureBinder> {
    let mut binder = FixtureBinder::build(fixture, binding, None, None);
    binder.fence = Some(Arc::clone(handle));
    Arc::new(binder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_entry_fence_admits_and_reports_the_use_entered() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let (fence, handle) = TestFence::new();
    let binder = fenced_binder(&fixture, Binding::Original, &handle);

    let sent = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect("admitted through the open fence");
    assert_eq!(fence.entered(), 1);
    assert_eq!(fence.outcomes(), vec![GovernedEntryOutcome::Entered]);
    assert_eq!(binder.admitted(), vec![sent.admission]);
    assert_eq!(binder.withdrawn(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_entry_fence_refuses_before_admission_with_its_exact_cause() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;

    // Expired and Unavailable are injected causes here: the fixture only
    // proves they stay distinct through native admission.
    for refusal in [
        GovernedEntryRefusal::Retired,
        GovernedEntryRefusal::Expired,
        GovernedEntryRefusal::Unavailable("transport lost".to_string()),
    ] {
        let (fence, handle) = TestFence::new();
        assert!(fence.close(refusal.clone()).await.is_empty());
        let binder = fenced_binder(&fixture, Binding::Original, &handle);
        let error = runtime
            .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
            .await
            .expect_err("a closed fence refuses the final entry");
        match error {
            IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(actual)) => {
                assert_eq!(actual, refusal, "the exact cause, never another");
            }
            other => panic!("expected an entry refusal, got {other}"),
        }
        // Unentered: nothing reached native admission, the custody was
        // withdrawn, no outcome was reported.
        assert!(binder.admitted().is_empty());
        assert_eq!(binder.withdrawn(), 1);
        assert_eq!(binder.retained(), 0);
        assert!(fence.outcomes().is_empty());
    }
    assert_eq!(fixture.adapter.gate.entered(), 0, "nothing was submitted");
    assert!(fixture.provider.requests().is_empty());
}

/// A retirement racing an entered use waits for that use's admission and
/// reports it entered; a sibling fence is independent of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retirement_waits_for_the_entered_use_and_a_sibling_stays_open() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let (login_a, handle_a) = TestFence::new();
    let (login_b, handle_b) = TestFence::new();

    fixture.adapter.gate.hold();
    let sending = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        let binder = fenced_binder(&fixture, Binding::Original, &handle_a);
        async move {
            runtime
                .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder)
                .await
        }
    });
    tokio::time::timeout(WAIT, async {
        while fixture.adapter.gate.entered() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the use entered native admission");
    assert_eq!(login_a.entered(), 1);

    // The retirement is polled and stays pending while the entered use's
    // admission is held.
    let retirement = login_a.close(GovernedEntryRefusal::Retired);
    tokio::pin!(retirement);
    for _ in 0..20 {
        assert!(
            futures::poll!(retirement.as_mut()).is_pending(),
            "the retirement waits for the entered use"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    fixture.adapter.gate.release();
    let sent = sending.await.expect("send task");
    assert!(sent.is_ok(), "entry won: admitted");
    assert_eq!(
        retirement.await,
        vec![GovernedEntryOutcome::Entered],
        "the retirement reports the use entered, never withdrawn"
    );

    // Login A is closed: a later use is unentered. Login B's fence is
    // independent and still grants entry. B binds the same event, so its
    // admission may be a native replay of A's input: this proves independent
    // fence permission, not fresh work.
    let after = fenced_binder(&fixture, Binding::Original, &handle_a);
    assert!(matches!(
        runtime
            .send_governed_with_turn_ticket(&identity, None, raw_prompt(), after.clone())
            .await,
        Err(IdentityRuntimeError::GovernedInput(
            GovernedInputError::EntryRefused(GovernedEntryRefusal::Retired)
        ))
    ));
    assert_eq!(after.withdrawn(), 1);
    let sibling = fenced_binder(&fixture, Binding::Original, &handle_b);
    runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), sibling)
        .await
        .expect("a sibling login's fence is unaffected");
    assert_eq!(login_b.outcomes(), vec![GovernedEntryOutcome::Entered]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fenced_use_reports_unentered_and_uncertain_from_the_native_settlement() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;

    // Native admission refuses it (NotAdmitted): unentered.
    let (refused, handle) = TestFence::new();
    let binder = fenced_binder(&fixture, Binding::WrongDestination, &handle);
    runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("native admission refuses the other destination");
    assert_eq!(refused.outcomes(), vec![GovernedEntryOutcome::Unentered]);
    assert_eq!(binder.withdrawn(), 1);

    // The admission attempt fails inside native admission (Uncertain): the
    // use is uncertain and the custody is retained, never withdrawn.
    fixture
        .adapter
        .panic_at_custody_read
        .store(3, Ordering::SeqCst);
    let (uncertain, handle) = TestFence::new();
    let binder = fenced_binder(&fixture, Binding::Original, &handle);
    runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect_err("the admission attempt failed");
    assert_eq!(uncertain.outcomes(), vec![GovernedEntryOutcome::Uncertain]);
    assert_eq!(binder.retained(), 1);
    assert_eq!(binder.withdrawn(), 0);
}

/// The caller leaving does not drop the reserved attempt: the runtime-owned
/// admission still decides and reports the use; a native replay of the same
/// event through another open fence reports Entered for the canonical input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_departed_caller_keeps_the_reserved_use_and_a_replay_is_entered() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let (first, handle) = TestFence::new();

    fixture.adapter.gate.hold();
    let caller = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let identity = identity.clone();
        let binder = fenced_binder(&fixture, Binding::Original, &handle);
        async move {
            runtime
                .send_governed_with_turn_ticket(&identity, None, raw_prompt(), binder)
                .await
        }
    });
    tokio::time::timeout(WAIT, async {
        while fixture.adapter.gate.entered() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the use entered native admission");
    caller.abort();
    assert!(caller.await.is_err(), "the caller is gone");
    fixture.adapter.gate.release();
    tokio::time::timeout(WAIT, async {
        while first.outcomes().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the runtime-owned admission reported its use");
    assert_eq!(first.outcomes(), vec![GovernedEntryOutcome::Entered]);

    let (replay, handle) = TestFence::new();
    let retry = fenced_binder(&fixture, Binding::Retry, &handle);
    let replayed = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), retry.clone())
        .await
        .expect("answered by native replay");
    assert!(
        replayed.admission.deduplicated,
        "a native replay, not fresh work"
    );
    assert_eq!(replay.outcomes(), vec![GovernedEntryOutcome::Entered]);
    assert_eq!(retry.withdrawn(), 1, "the retry custody is withdrawn");
}

/// The trusted host's governed dispatch reaches its own member in the shape
/// neither door alone allows: an `InternalOnly` Meta identity on a member
/// whose profile is not externally addressable. The governed send to that
/// identity is refused before the binder is invoked; the dispatch through
/// the internal door is bound with the member session's actual controller,
/// admitted with its bound claims retained, settled exactly once and
/// completes under its ticket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_host_dispatch_reaches_an_internal_only_identity_on_a_non_addressable_member() {
    let fixture = Arc::new(Fixture::non_addressable().await);
    let (runtime, identity) =
        governed_runtime_composed(&fixture, None, None, AgentAddressability::InternalOnly).await;
    let registered = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");

    let send_binder = FixtureBinder::authored_from(&fixture, registered.spec.clone());
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), send_binder.clone())
        .await
        .expect_err("the external door refuses an internal-only identity");
    assert!(
        matches!(error, IdentityRuntimeError::NotAddressable(_)),
        "{error}"
    );
    assert!(send_binder.rows().is_empty(), "refused before the binder");
    assert!(send_binder.admitted().is_empty());
    assert_eq!(send_binder.withdrawn(), 0, "no custody was ever created");

    let binder = FixtureBinder::authored_from(&fixture, registered.spec.clone());
    let sent = runtime
        .dispatch_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect("the trusted host's dispatch is admitted");
    assert_eq!(
        binder.contexts.lock().expect("contexts").as_slice(),
        [(fixture.session_id.clone(), selection())]
    );
    assert_eq!(binder.rows(), vec![registered.spec]);
    let bound_input = binder
        .bound_input
        .lock()
        .expect("bound input")
        .clone()
        .expect("bound");
    assert_eq!(sent.admission.input_id, bound_input);
    assert!(!sent.admission.deduplicated);
    assert_eq!(binder.admitted(), vec![sent.admission.clone()]);
    assert_eq!(binder.withdrawn(), 0);
    assert!(sent.settlement.is_ok());
    let stored = fixture
        .machine
        .input_state(&fixture.session_id, &bound_input)
        .await
        .expect("native row query")
        .expect("the member session admitted exactly the bound input");
    assert_eq!(stored.state.authority_contributors.len(), 1);
    assert_eq!(
        stored.state.authority_contributors[0].input_id(),
        &bound_input
    );
    let bound = InputAuthorityAssociation::new(association(
        &fixture.runtime,
        fixture.controller.clone(),
        fixture.operation.clone(),
        selection(),
    ))
    .expect("claims");
    assert!(
        stored.state.authority_contributors[0].association() == &bound,
        "the member retained the host's bound claims, unaltered"
    );

    let TurnTracking::Tracked(ticket) = sent.ticketed.turn else {
        panic!("a governed dispatch is tracked by its own ticket");
    };
    let outcome = runtime.wait_for_turn(&identity, ticket, WAIT).await;
    assert!(completed_with_answer(&outcome), "{outcome:?}");
    let requests = fixture.provider.requests();
    assert_eq!(requests.len(), 1, "one native controller request");
    assert!(mentions_prompt(&requests[0]));
    assert_eq!(binder.admitted().len(), 1, "settled once");
}

/// The shape of a host's setup helper: an addressable Meta identity on a
/// member whose profile is not externally addressable. The governed send is
/// bound and then refused by the mob as not externally addressable, with its
/// custody withdrawn and nothing run; the trusted host's dispatch reaches
/// the same member and is settled exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_host_dispatch_reaches_an_addressable_identity_on_a_non_addressable_member() {
    let fixture = Arc::new(Fixture::non_addressable().await);
    let (runtime, identity) = governed_runtime(&fixture, None).await;
    let registered = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");

    let send_binder = FixtureBinder::authored_from(&fixture, registered.spec.clone());
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, raw_prompt(), send_binder.clone())
        .await
        .expect_err("the external door refuses a member that is not externally addressable");
    assert!(
        matches!(
            &error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(mob))
                if matches!(**mob, MobError::NotExternallyAddressable(_))
        ),
        "{error}"
    );
    assert!(send_binder.admitted().is_empty());
    assert_eq!(send_binder.withdrawn(), 1, "the custody is withdrawn");
    assert!(fixture.provider.requests().is_empty(), "nothing ran");

    let binder = FixtureBinder::authored_from(&fixture, registered.spec.clone());
    let sent = runtime
        .dispatch_governed_with_turn_ticket(&identity, None, raw_prompt(), binder.clone())
        .await
        .expect("the trusted host's dispatch is admitted");
    assert_eq!(binder.admitted(), vec![sent.admission.clone()]);
    assert_eq!(binder.withdrawn(), 0);
    assert!(sent.settlement.is_ok());
    let TurnTracking::Tracked(ticket) = sent.ticketed.turn else {
        panic!("a governed dispatch is tracked by its own ticket");
    };
    let outcome = runtime.wait_for_turn(&identity, ticket, WAIT).await;
    assert!(completed_with_answer(&outcome), "{outcome:?}");
    assert_eq!(fixture.provider.requests().len(), 1);
}
