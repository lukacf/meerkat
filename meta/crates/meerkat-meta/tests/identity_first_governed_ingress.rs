#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A trusted governed input reaches its member only through the identity
//! runtime's governed send: under the identity's lease, bound by the host for
//! the member's exact current session, and never delivered unbound, ungoverned
//! or on another lane.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BoundGovernedInput,
    BridgeAdmissionError, BridgeDelivery, BridgeError, DurabilityPolicy, DurableAgentSpec,
    GovernedBindContext, GovernedDelivery, GovernedInputBinder, GovernedInputError,
    IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError, LocalContinuityStore,
    LocalLeaseProvider, MemberInspection, MemberRuntimeBinding, ResumeSessionOutcome,
    SessionBridge, SessionSnapshot,
};

#[path = "support/core_members.rs"]
mod core_members;

use core_members::CoreMembers;

/// A bridge that counts every delivery on every lane. Core's member reads
/// answer from [`CoreMembers`].
#[derive(Default)]
struct CountingBridge {
    ordinary: AtomicUsize,
    governed: AtomicUsize,
    core: CoreMembers,
}

#[async_trait]
impl SessionBridge for CountingBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let binding = MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(meerkat_core::types::SessionId::new()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.core.bind(
            identity,
            &MemberRuntimeBinding {
                agent_runtime_id: agent_runtime_id.clone(),
                session_id: Some(session_id.clone()),
            },
            &spec.profile,
        );
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        self.ordinary.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::Mob(
            "no ordinary delivery is expected".to_string(),
        ))
    }

    async fn begin_governed_delivery(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
        _input: meerkat_runtime::Input,
        _custody: meerkat_meta::identity_first::GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.governed.fetch_add(1, Ordering::SeqCst);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported("counted".to_string()),
        ))
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn inspect_member(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        Ok(MemberInspection {
            output_preview: None,
            preview_unavailable: None,
            is_final: false,
            peer_reachable_count: 0,
        })
    }
}

/// A host binder that counts every bind it is asked for.
#[derive(Default)]
struct CountingBinder {
    binds: AtomicUsize,
}

#[async_trait]
impl GovernedInputBinder for CountingBinder {
    async fn bind(
        &self,
        _context: &GovernedBindContext,
        _input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, String> {
        self.binds.fetch_add(1, Ordering::SeqCst);
        Err("unexpected bind".to_string())
    }

    fn retain_unresolved(
        &self,
        _settlement: Box<dyn meerkat_meta::identity_first::GovernedSettlement>,
        _detail: &str,
    ) {
        panic!("nothing is submitted without a pinned controller");
    }
}

fn make_runtime(bridge: Arc<dyn SessionBridge>) -> Arc<IdentityRuntime> {
    let store: Arc<dyn ContinuityStore> = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "governed-ingress-test".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }))
}

/// Register `name` with Core holding its member live on `session` at
/// generation 0 under fence 1.
async fn register(
    runtime: &IdentityRuntime,
    bridge: &CountingBridge,
    name: &str,
    session: &meerkat_core::types::SessionId,
) -> AgentIdentity {
    let identity = AgentIdentity::parse(name).unwrap();
    let profile = meerkat_mob::ProfileName::from("default");
    bridge
        .core
        .bind_live_at_fence(&identity, &profile, 0, Some(session.clone()), 1);
    runtime
        .register(DurableAgentSpec {
            identity: identity.clone(),
            profile,
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        })
        .await;
    identity
}

fn prompt(text: &str) -> meerkat_runtime::Input {
    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(text, None))
}

#[tokio::test]
async fn a_member_without_a_pinned_controller_is_refused_before_binding_or_delivery() {
    // A bridge with no controller pin source cannot hand the host the
    // member session actor's actual controller: the send is refused before
    // the binder is asked and before anything is delivered on any lane.
    let bridge = Arc::new(CountingBridge::default());
    let runtime = make_runtime(bridge.clone());
    let session = meerkat_core::types::SessionId::new();
    let identity = register(&runtime, &bridge, "governed-member", &session).await;
    let binder = Arc::new(CountingBinder::default());
    let error = runtime
        .send_governed_with_turn_ticket(&identity, None, prompt("hello"), binder.clone())
        .await
        .expect_err("no pinned controller");
    assert!(
        matches!(
            error,
            IdentityRuntimeError::GovernedInput(GovernedInputError::Unsupported(_))
        ),
        "{error}"
    );
    assert_eq!(binder.binds.load(Ordering::SeqCst), 0);
    assert_eq!(bridge.ordinary.load(Ordering::SeqCst), 0);
    assert_eq!(bridge.governed.load(Ordering::SeqCst), 0);
}
