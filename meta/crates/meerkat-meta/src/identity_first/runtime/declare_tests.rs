//! Rows for a registration's declaration in Core's mob over session bridges
//! with no Core mob: a bridge without Core's declared apply declares nothing
//! and registers nothing, and another apply of the mob in progress is waited
//! out within a bounded time. And rows for a door's declaration that Core
//! leaves to its next apply: the door waits for the lifecycle operation of
//! its member in flight, within the same bound, and declares it again.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::types::SessionId;

use super::*;
use crate::identity_first::bridge::{
    BridgeDelivery, DeclaredRosterAnswer, MemberRuntimeBinding, ResumeSessionOutcome,
};
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::{
    AgentAddressability, AgentRuntimeId, DurabilityPolicy, IdentityRuntimeConfig, SessionSnapshot,
};

/// A session bridge whose `apply_declared_roster` is the trait's default.
struct NoDeclaredApplyBridge;

#[async_trait::async_trait]
impl SessionBridge for NoDeclaredApplyBridge {
    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Mob("no delivery in this test".to_string()))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob("no checkpoint in this test".to_string()))
    }

    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Mob(
            "no member is created in this test".to_string(),
        ))
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Mob(
            "no member is resumed in this test".to_string(),
        ))
    }

    async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// A session bridge whose first `pending` declared applies meet another
/// apply of the mob in progress; every later one keeps each declared
/// identity.
struct PendingApplyBridge {
    pending: usize,
    applies: AtomicUsize,
}

impl PendingApplyBridge {
    fn new(pending: usize) -> Self {
        Self {
            pending,
            applies: AtomicUsize::new(0),
        }
    }

    fn applies(&self) -> usize {
        self.applies.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SessionBridge for PendingApplyBridge {
    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Mob("no delivery in this test".to_string()))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob("no checkpoint in this test".to_string()))
    }

    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Mob(
            "no member is created in this test".to_string(),
        ))
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Mob(
            "no member is resumed in this test".to_string(),
        ))
    }

    async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn apply_declared_roster(
        &self,
        roster: DeclaredRosterRequest,
    ) -> Result<DeclaredRosterAnswer, BridgeError> {
        if self.applies.fetch_add(1, Ordering::SeqCst) < self.pending {
            return Err(BridgeError::DeclaredApplyPending {
                detail: "another apply of the mob is in progress".to_string(),
            });
        }
        Ok(DeclaredRosterAnswer::kept_for(&roster))
    }
}

fn runtime_over(bridge: Arc<dyn SessionBridge>) -> IdentityRuntime {
    IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(
            LocalContinuityStore::in_memory().expect("an in-memory continuity store"),
        ),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "declare-rows".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    })
}

fn keeper() -> AgentIdentity {
    AgentIdentity::parse("keeper").expect("a valid identity")
}

fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// A registration through a session bridge that cannot reach Core's
/// declared apply declares nothing. It is refused, typed and not
/// retryable, and registers nothing, so no Meta verb goes ahead on a spec
/// Core never decided.
///
/// RED (`RED-1b-default-keeps.patch`): the trait default answers every
/// identity kept, and the registration succeeds.
#[tokio::test]
async fn a_registration_through_a_bridge_without_the_declared_apply_registers_nothing() {
    let runtime = runtime_over(Arc::new(NoDeclaredApplyBridge));
    let keeper = keeper();

    let error = runtime
        .register(spec(&keeper))
        .await
        .expect_err("nothing was declared, so nothing registers");
    assert!(
        matches!(
            &error,
            IdentityRuntimeError::DeclarationNotApplied {
                retryable: false,
                detail,
            } if detail.contains("apply_declared_roster")
        ),
        "{error:?}"
    );
    assert_eq!(
        error.structured_data(),
        Some(serde_json::json!({
            "kind": "identity_declaration_not_applied", "retryable": false,
        }))
    );
    assert!(
        matches!(
            runtime.registered_spec(&keeper).await,
            Err(IdentityRuntimeError::UnknownIdentity(_))
        ),
        "the refused registration registered no spec"
    );
}

/// A registration that meets another apply of the mob in progress is
/// declared again, with backoff, until Core answers it.
///
/// RED (`RED-1b-pending-not-retried.patch`): a pending apply is answered as
/// not retryable, and the first one refuses the registration.
#[tokio::test(start_paused = true)]
async fn a_registration_that_meets_another_apply_is_declared_again_until_core_answers() {
    let bridge = Arc::new(PendingApplyBridge::new(3));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();

    let answer = runtime
        .register(spec(&keeper))
        .await
        .expect("Core answers once the other apply ends");
    assert!(matches!(answer, DeclaredIdentityAnswer::Kept), "{answer:?}");
    assert_eq!(
        bridge.applies(),
        4,
        "three applies met the other apply, and the fourth was answered"
    );
    assert!(
        runtime.registered_spec(&keeper).await.is_ok(),
        "the answered declaration registers the spec"
    );
}

/// An apply of the mob that stays in progress past the bounded wait
/// refuses the registration, retryably, with nothing registered.
#[tokio::test(start_paused = true)]
async fn a_registration_that_never_gets_past_another_apply_is_refused_retryably() {
    let bridge = Arc::new(PendingApplyBridge::new(usize::MAX));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();
    let started = Instant::now();

    let error = runtime
        .register(spec(&keeper))
        .await
        .expect_err("the other apply never ends");
    assert!(
        matches!(
            error,
            IdentityRuntimeError::DeclarationNotApplied {
                retryable: true,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        started.elapsed() <= DECLARED_APPLY_PENDING_WAIT,
        "the wait is bounded: {:?}",
        started.elapsed()
    );
    assert!(
        bridge.applies() > 1,
        "the identity was declared again while it waited"
    );
    assert!(
        matches!(
            runtime.registered_spec(&keeper).await,
            Err(IdentityRuntimeError::UnknownIdentity(_))
        ),
        "the refused registration registered no spec"
    );
}

/// The applies and the waits a deferring bridge saw.
#[derive(Default)]
struct DeferralCounts {
    applies: AtomicUsize,
    /// The timeout of each wait for a member's lifecycle to settle.
    waits: std::sync::Mutex<Vec<Duration>>,
}

impl DeferralCounts {
    fn applies(&self) -> usize {
        self.applies.load(Ordering::SeqCst)
    }

    fn waits(&self) -> Vec<Duration> {
        self.waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Core's answer to a declared apply while `deferring` applies are still to
/// defer: each declared identity left to its next apply, as Core leaves a
/// member whose lifecycle operation is in flight; every identity kept
/// otherwise.
fn deferring_answer(
    roster: &DeclaredRosterRequest,
    deferring: &AtomicUsize,
    counts: &DeferralCounts,
) -> DeclaredRosterAnswer {
    counts.applies.fetch_add(1, Ordering::SeqCst);
    let mut answer = DeclaredRosterAnswer::kept_for(roster);
    let deferred = deferring
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
            left.checked_sub(1)
        })
        .is_ok();
    if deferred {
        for outcome in answer.identities.values_mut() {
            *outcome = DeclaredIdentityAnswer::Deferred {
                reason: BridgeError::Mob(
                    "mob lifecycle operation is still in progress: the member's wake is in \
                     flight"
                        .to_string(),
                ),
            };
        }
    }
    answer
}

/// A session bridge whose next `deferring` declared applies leave each
/// identity to its next apply, and whose wait for a member's lifecycle to
/// settle answers `settlement`.
struct LifecycleBridge {
    deferring: AtomicUsize,
    settlement: meerkat_mob::MemberLifecycleSettlement,
    counts: DeferralCounts,
}

impl LifecycleBridge {
    fn new(settlement: meerkat_mob::MemberLifecycleSettlement) -> Self {
        Self {
            deferring: AtomicUsize::new(0),
            settlement,
            counts: DeferralCounts::default(),
        }
    }

    /// The next `applies` declared applies leave each identity to its next
    /// apply.
    fn defer(&self, applies: usize) {
        self.deferring.store(applies, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SessionBridge for LifecycleBridge {
    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Mob("no delivery in this test".to_string()))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob("no checkpoint in this test".to_string()))
    }

    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Mob(
            "no member is created in this test".to_string(),
        ))
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Mob(
            "no member is resumed in this test".to_string(),
        ))
    }

    async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn apply_declared_roster(
        &self,
        roster: DeclaredRosterRequest,
    ) -> Result<DeclaredRosterAnswer, BridgeError> {
        Ok(deferring_answer(&roster, &self.deferring, &self.counts))
    }

    async fn wait_member_lifecycle_settled(
        &self,
        _identity: &AgentIdentity,
        timeout: Duration,
    ) -> Result<meerkat_mob::MemberLifecycleSettlement, BridgeError> {
        self.counts
            .waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(timeout);
        Ok(self.settlement)
    }
}

/// A deferring session bridge whose wait for a member's lifecycle is the
/// trait's default.
#[derive(Default)]
struct NoLifecycleWaitBridge {
    deferring: AtomicUsize,
    counts: DeferralCounts,
}

#[async_trait::async_trait]
impl SessionBridge for NoLifecycleWaitBridge {
    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Mob("no delivery in this test".to_string()))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob("no checkpoint in this test".to_string()))
    }

    async fn create_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Mob(
            "no member is created in this test".to_string(),
        ))
    }

    async fn resume_session(
        &self,
        _identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Mob(
            "no member is resumed in this test".to_string(),
        ))
    }

    async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn apply_declared_roster(
        &self,
        roster: DeclaredRosterRequest,
    ) -> Result<DeclaredRosterAnswer, BridgeError> {
        Ok(deferring_answer(&roster, &self.deferring, &self.counts))
    }
}

/// `keeper`'s door declaration, under its lifecycle lock, as a door holds it.
async fn door_declaration(
    runtime: &IdentityRuntime,
    keeper: &AgentIdentity,
) -> Result<SeatOutcome, IdentityRuntimeError> {
    let lifecycle_lock = runtime.lifecycle_lock_for(keeper).await;
    let _lifecycle_guard = lifecycle_lock.lock().await;
    runtime.declare_identity_locked(&spec(keeper), None).await
}

/// Whether `error` is the door's retryable refusal of a declaration Core left
/// to its next apply.
fn left_to_next_apply(error: &IdentityRuntimeError) -> bool {
    matches!(
        error,
        IdentityRuntimeError::IdentityWakeRetryable { detail, .. }
            if detail.contains("left the identity's declaration to its next apply")
    )
}

/// A door's declaration that Core leaves to its next apply, because a
/// lifecycle operation of the member is in flight, waits for that operation
/// to end and declares the identity again: Core's answer on the settled
/// member (here, the seat kept) is the door's. The wait is bounded by the
/// door's re-declaration bound.
#[tokio::test]
async fn a_door_declaration_left_to_the_next_apply_waits_out_the_member_lifecycle() {
    let bridge = Arc::new(LifecycleBridge::new(
        meerkat_mob::MemberLifecycleSettlement::Settled,
    ));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();
    runtime
        .register(spec(&keeper))
        .await
        .expect("Core keeps the registered identity");
    let registered = bridge.counts.applies();

    bridge.defer(1);
    let seat = door_declaration(&runtime, &keeper)
        .await
        .expect("the door acts on the member once its lifecycle operation ended");
    assert!(seat.already_seated, "Core kept the settled member's seat");
    assert_eq!(
        bridge.counts.applies() - registered,
        2,
        "the declaration Core left to its next apply was declared again once"
    );
    let waits = bridge.counts.waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert!(
        waits[0] > Duration::ZERO && waits[0] <= DECLARED_APPLY_PENDING_WAIT,
        "the wait is bounded by the door's re-declaration bound: {waits:?}"
    );
}

/// A member whose lifecycle operation is still in flight when the door's
/// bound runs out leaves the door's answer as Core gave it: the retryable
/// refusal, after one declaration and one wait.
#[tokio::test]
async fn a_door_whose_member_stays_in_flight_answers_the_retryable_refusal() {
    let bridge = Arc::new(LifecycleBridge::new(
        meerkat_mob::MemberLifecycleSettlement::StillInFlight,
    ));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();
    runtime
        .register(spec(&keeper))
        .await
        .expect("Core keeps the registered identity");
    let registered = bridge.counts.applies();

    bridge.defer(usize::MAX);
    let Err(error) = door_declaration(&runtime, &keeper).await else {
        panic!("the member's lifecycle operation stays in flight");
    };
    assert!(left_to_next_apply(&error), "{error:?}");
    assert_eq!(bridge.counts.applies() - registered, 1);
    assert_eq!(bridge.counts.waits().len(), 1);
}

/// A deferral while Core holds no lifecycle operation of the member in
/// flight (the operation ended before the wait read it, or Core left the
/// declaration for another cause) is declared again once, never more: a
/// deferral no operation's end clears is answered at once.
#[tokio::test]
async fn a_door_deferral_with_nothing_in_flight_is_declared_again_once() {
    let bridge = Arc::new(LifecycleBridge::new(
        meerkat_mob::MemberLifecycleSettlement::NothingInFlight,
    ));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();
    runtime
        .register(spec(&keeper))
        .await
        .expect("Core keeps the registered identity");
    let registered = bridge.counts.applies();

    bridge.defer(usize::MAX);
    let Err(error) = door_declaration(&runtime, &keeper).await else {
        panic!("Core leaves every declaration to its next apply");
    };
    assert!(left_to_next_apply(&error), "{error:?}");
    assert_eq!(bridge.counts.applies() - registered, 2);
    assert_eq!(bridge.counts.waits().len(), 2);

    bridge.defer(1);
    door_declaration(&runtime, &keeper)
        .await
        .expect("the declaration made again once is Core's answer");
}

/// Only a door waits: a registration answers Core's deferral as Core gave it,
/// and a door through a bridge that cannot read Core's member lifecycle (the
/// trait's default) answers its refusal after one declaration.
#[tokio::test]
async fn a_registration_and_a_bridge_without_the_wait_answer_a_deferral_at_once() {
    let bridge = Arc::new(LifecycleBridge::new(
        meerkat_mob::MemberLifecycleSettlement::Settled,
    ));
    let runtime = runtime_over(bridge.clone());
    let keeper = keeper();
    bridge.defer(1);
    let answer = runtime
        .register(spec(&keeper))
        .await
        .expect("the registration answers Core's deferral");
    assert!(
        matches!(answer, DeclaredIdentityAnswer::Deferred { .. }),
        "{answer:?}"
    );
    assert_eq!(bridge.counts.applies(), 1);
    assert!(
        bridge.counts.waits().is_empty(),
        "a registration waits for nothing"
    );

    let bridge = Arc::new(NoLifecycleWaitBridge::default());
    let runtime = runtime_over(bridge.clone());
    runtime
        .register(spec(&keeper))
        .await
        .expect("Core keeps the registered identity");
    let registered = bridge.counts.applies();
    bridge.deferring.store(usize::MAX, Ordering::SeqCst);
    let Err(error) = door_declaration(&runtime, &keeper).await else {
        panic!("Core leaves the declaration to its next apply");
    };
    assert!(left_to_next_apply(&error), "{error:?}");
    assert_eq!(bridge.counts.applies() - registered, 1);
}
