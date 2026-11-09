//! A host's settle of a member's retained run when the member's runtime
//! registration is durability-degraded. The member runtime's settle verb
//! decides: a registration whose store realm lost its fence is refused as a
//! session that leaves this process at every call, the first and every retry
//! alike, and any other degraded registration answers the reload refusal.

use super::*;

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// A member identity of its own: test gates and in-process routes are keyed
/// by identity and mob id, and tests share one process.
fn unique_identity(label: &str) -> AgentIdentity {
    AgentIdentity::from(format!("{label}-{}", uuid::Uuid::new_v4().simple()))
}

/// A store realm's fence lost for good (another writer took it, or the realm
/// stopped itself); its window still reads open.
struct LostFence;

#[async_trait::async_trait]
impl meerkat_core::host_store::SelfFence for LostFence {
    fn window(&self) -> meerkat_core::host_store::SelfFenceWindow {
        meerkat_core::host_store::SelfFenceWindow {
            elapsed: Duration::from_secs(1),
            window: Duration::from_secs(2),
            renew_failed: false,
        }
    }

    fn lost(&self) -> bool {
        true
    }
}

/// Two members of one mob over a persistent machine whose in-memory store
/// stands in for a host store realm, both registrations durability-degraded,
/// as a realm that lost its fence leaves the registration whose teardown it
/// refused. While the realm holds its fence, a settle of a degraded member
/// answers the reload refusal, `MemberReloadRequired`. Once the realm lost
/// its fence, a settle of the other member answers `MemberSessionNotLive` at
/// the first call and at the host's retry, and never sends the host to a
/// reload no call in this process can make. RED before: the mob verb refused
/// every degraded registration itself, as `MemberReloadRequired`, before the
/// runtime verb could tell a lost fence apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_degraded_member_whose_store_realm_lost_its_fence_is_not_live_at_every_settle() {
    let label = "degraded-settle";
    let memory = Arc::new(InMemoryRuntimeStore::new());
    let runtime_store: Arc<dyn RuntimeStore> = Arc::clone(&memory) as Arc<dyn RuntimeStore>;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(Arc::clone(&adapter));
    let handle = MobBuilder::new(
        with_unique_mob_id(turn_driven_definition(), label),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create mob");
    let mut members = Vec::new();
    for _ in 0..2 {
        let identity = unique_identity(label);
        let session_id = handle
            .spawn(ProfileName::from("worker"), identity.clone(), None)
            .await
            .expect("spawn member")
            .bridge_session_id()
            .expect("runtime-backed member has a bridge session")
            .clone();
        assert!(
            adapter
                .force_session_durability_reload_required_for_test(&session_id)
                .await,
            "the member's registration degrades"
        );
        members.push((identity, session_id));
    }
    let (degraded, _) = &members[0];
    let (fenced, fenced_session) = &members[1];

    let refused = tokio::time::timeout(STEP, handle.settle_member_retained_run(degraded))
        .await
        .expect("the settle answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::MemberReloadRequired { member_id, .. }) if member_id == degraded
        ),
        "a degraded registration whose store realm holds its fence answers the reload \
         refusal, not {refused:?}"
    );

    memory.set_self_fence_for_test(Some(Arc::new(LostFence)));
    for attempt in ["the first settle", "the host's retry"] {
        assert!(
            adapter
                .durability_reload_required(fenced_session)
                .await
                .is_some(),
            "[{attempt}] the member's registration reads durability-degraded"
        );
        let refused = tokio::time::timeout(STEP, handle.settle_member_retained_run(fenced))
            .await
            .expect("the settle answers");
        assert!(
            matches!(
                &refused,
                Err(MobError::MemberSessionNotLive { agent_identity, session_id })
                    if agent_identity == fenced && session_id == fenced_session
            ),
            "[{attempt}] a member whose store realm lost its fence answers \
             MemberSessionNotLive, never MemberReloadRequired, not {refused:?}"
        );
    }
}
