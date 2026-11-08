//! Rows for a registration's declaration in Core's mob over session bridges
//! with no Core mob: a bridge without Core's declared apply declares nothing
//! and registers nothing, and another apply of the mob in progress is waited
//! out within a bounded time.

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
