//! Rows for the identity views over Core's entry read: every view of an
//! identity reads Core's facts (its member, whether Core holds it Broken,
//! the host preparation it owes), never a note this process kept, and a
//! read Core could not serve refuses typed instead of standing in for a
//! fact.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;

use crate::identity_first::bridge::{CoreBrokenMember, ResumeRejectionKind};
use crate::identity_first::core_test_bridge::{CoreReadsBridge, bind_live, register_live};
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::{
    AgentAddressability, AgentIdentity, DurabilityPolicy, DurableAgentSpec, IdentityLifecycleState,
    IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError,
};

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("valid identity")
}

fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: "worker".into(),
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

/// An identity runtime over `bridge`'s fake Core members: one process.
fn runtime_over(bridge: &Arc<CoreReadsBridge>, instance: &str) -> Arc<IdentityRuntime> {
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    }))
}

/// A final wake refusal, then a restart: the identity's status reads the
/// same before and after it, because every field of it is Core's. Core
/// refused the wake of the identity's member for good and held the member
/// Broken; then a host respawned the member outside Meta's verbs, a new
/// incarnation on a new session that Core does not hold Broken. The process
/// that saw the refusal reads that incarnation as Core holds it, and so does
/// a process started afterwards over the same Core.
///
/// RED before the views read Core's entry: the process that saw the refusal
/// kept it (`wake_refusal`) until its own next wake, so its status still
/// carried the final refusal of an incarnation Core had replaced, and the
/// restarted process's status did not.
#[tokio::test]
async fn a_final_wake_refusal_then_a_restart_reads_the_same_state() {
    let bridge = Arc::new(CoreReadsBridge::default());
    let alice = id("views-restart");
    let before = runtime_over(&bridge, "views-restart-before");
    register_live(&before, &bridge.core, spec(&alice), 1, SessionId::new())
        .await
        .expect("a live registered member");
    // Core holds the member Broken and refuses its wake for good.
    bridge.core.mark_broken(
        &alice,
        CoreBrokenMember::new("the member's session cannot be resumed"),
    );
    bridge.refuse_wakes_of(
        &alice,
        ResumeRejectionKind::Unavailable {
            reason: meerkat_mob::error::SessionResumeUnavailableReason::ArchivedNotRevivable,
        },
    );
    let refused = before.materialize(&alice).await;
    assert!(
        matches!(
            refused,
            Err(IdentityRuntimeError::IdentityUnresumable { .. })
        ),
        "Core's final refusal of the wake is answered typed: {refused:?}"
    );
    // A host respawns the member outside Meta's verbs: a new incarnation on
    // a new session, which Core does not hold Broken.
    bridge.allow_wakes_of(&alice);
    bind_live(
        &bridge.core,
        &alice,
        &spec(&alice).profile,
        2,
        Some(SessionId::new()),
    )
    .expect("the respawned member");
    let read_before = before
        .status(&alice)
        .await
        .expect("the status before the restart");

    let after = runtime_over(&bridge, "views-restart-after");
    after
        .register(spec(&alice))
        .await
        .expect("the restarted process registers the identity");
    let read_after = after
        .status(&alice)
        .await
        .expect("the status after the restart");

    assert_eq!(
        read_before, read_after,
        "a restart changes no field of the status: every field is Core's"
    );
    assert_eq!(read_after.state, IdentityLifecycleState::Active);
    assert_eq!(read_after.wake_refusal, None);
    assert_eq!(
        read_after.generation.map(meerkat_mob::Generation::get),
        Some(2)
    );
}

/// A session bridge that reads Core's member history but not Core's entry
/// of an identity, so not whether Core holds its member Broken: every view
/// of the identity refuses typed and retryable (history unavailable) instead
/// of reading the member active on a read that could not tell.
///
/// RED before the views read Core's entry: Core's Broken members were a
/// read of their own whose default, for a bridge without it, was none, so
/// the identity read active while Core held its member Broken.
#[tokio::test]
async fn a_bridge_without_the_broken_read_refuses_instead_of_reading_active() {
    let bridge = Arc::new(CoreReadsBridge::default());
    bridge.refuse_entry_reads();
    let runtime = runtime_over(&bridge, "views-no-entry-read");
    let alice = id("views-no-entry-read");
    register_live(&runtime, &bridge.core, spec(&alice), 1, SessionId::new())
        .await
        .expect("a live registered member");
    bridge
        .core
        .mark_broken(&alice, CoreBrokenMember::new("the member's restore failed"));

    let status = runtime.status(&alice).await;
    assert!(
        matches!(
            status,
            Err(IdentityRuntimeError::IdentityHistoryUnavailable { .. })
        ),
        "the status refuses typed instead of reading the member active: {status:?}"
    );
    let statuses = runtime.statuses().await;
    assert!(
        matches!(
            statuses,
            Err(IdentityRuntimeError::MemberHistoryUnavailable { .. })
        ),
        "the status list refuses typed: {statuses:?}"
    );
    let health = runtime.member_health(&alice).await;
    assert!(
        matches!(
            health,
            Err(IdentityRuntimeError::IdentityHistoryUnavailable { .. })
        ),
        "the member health refuses typed: {health:?}"
    );
}

/// An identity's status while its runtime shuts down reads Core's facts:
/// Core still holds the member, so the identity reads active, with its
/// runtime id and its session. The runtime's own shutdown is no fact of the
/// identity.
///
/// RED before the views read Core alone: once this runtime began shutting
/// down, the view read a member Core holds as dormant.
#[tokio::test]
async fn status_during_shutdown_projects_core_not_a_flag() {
    let bridge = Arc::new(CoreReadsBridge::default());
    let runtime = runtime_over(&bridge, "views-shutdown");
    let alice = id("views-shutdown");
    let session = SessionId::new();
    let runtime_id = register_live(&runtime, &bridge.core, spec(&alice), 1, session.clone())
        .await
        .expect("a live registered member");

    runtime.close_foreground_operations_for_drain();

    let status = runtime.status(&alice).await.expect("the status reads");
    assert_eq!(
        status.state,
        IdentityLifecycleState::Active,
        "Core holds the member: {status:?}"
    );
    assert_eq!(status.agent_runtime_id.as_ref(), Some(&runtime_id));
    assert_eq!(status.session_id.as_ref(), Some(&session));
    let health = runtime
        .member_health(&alice)
        .await
        .expect("the member health reads");
    assert_eq!(health.state, IdentityLifecycleState::Active, "{health:?}");
}
