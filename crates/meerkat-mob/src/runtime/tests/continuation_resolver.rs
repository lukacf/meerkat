//! #1497 member continuations resolve through the mob roster: an address names
//! one incarnation (identity at generation), served by its current bridge
//! session, and retired by a respawn or a retirement.

use super::*;
use crate::continuation::{MobContinuationResolver, MobHandleLookup};
use meerkat::{
    AddressResolution, ContinuationAddressResolver, ContinuationDelivery, ContinuationHandling,
    ContinuationKey, ContinuationOwner, ContinuationOwnerService, ContinuationProducer,
    ContinuationResultRef, ContinuationStatus, ContinuationSubmitError, StrandedCause,
};
use meerkat_runtime::{InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryInbox};

struct OneMob(MobHandle);

#[async_trait::async_trait]
impl MobHandleLookup for OneMob {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle> {
        (self.0.mob_id() == mob_id).then(|| self.0.clone())
    }
}

fn delivery(key: &str) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(key).expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::Host {
                namespace: "tasks".into(),
            },
            producer_id: "op-1".into(),
            result_digest: "sha256:result".into(),
            summary: None,
        },
        body: "result".into(),
        handling: ContinuationHandling::Queue,
    }
}

#[tokio::test]
async fn member_continuations_follow_the_incarnation_across_respawn_and_retirement() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    let resolver = Arc::new(MobContinuationResolver::new(Arc::new(OneMob(
        handle.clone(),
    ))));
    let continuations = ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        resolver.clone(),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );
    let owner = ContinuationOwner::Member {
        mob_id: handle.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };

    let first_session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("bridge session");
    let first = handle
        .submit_continuation(&continuations, &identity, delivery("task-1"), 1)
        .await
        .expect("submit at the first incarnation");
    let first_address = LogicalRuntimeId::new(first.address.clone());
    assert_eq!(
        resolver.current_address(&owner).await.expect("current"),
        Some(first_address.clone())
    );
    assert_eq!(
        resolver
            .resolve_address(&first_address)
            .await
            .expect("resolve"),
        AddressResolution::Session(first_session.clone()),
        "the live incarnation is served by its bridge session"
    );

    handle
        .respawn(identity.clone(), None)
        .await
        .expect("respawn");
    assert_eq!(
        resolver
            .resolve_address(&first_address)
            .await
            .expect("resolve"),
        AddressResolution::Retired,
        "a respawn retires the earlier incarnation's address"
    );
    assert_eq!(
        continuations
            .continuation_status(&owner, &ContinuationKey::new("task-1").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::Stranded {
            receipt: first.clone(),
            cause: StrandedCause::OwnerRetired
        }
    );
    assert_eq!(
        handle
            .submit_continuation(&continuations, &identity, delivery("task-1"), 2)
            .await
            .expect("replay"),
        first,
        "an old key replays its first incarnation"
    );
    let second = handle
        .submit_continuation(&continuations, &identity, delivery("task-2"), 3)
        .await
        .expect("a new key binds the successor");
    assert_ne!(second.address, first.address);
    let second_session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("successor session");
    assert_ne!(second_session, first_session);
    assert_eq!(
        resolver
            .resolve_address(&LogicalRuntimeId::new(second.address.clone()))
            .await
            .expect("resolve"),
        AddressResolution::Session(second_session)
    );

    handle.retire(identity.clone()).await.expect("retire");
    assert_eq!(
        resolver.current_address(&owner).await.expect("current"),
        None
    );
    assert_eq!(
        handle
            .submit_continuation(&continuations, &identity, delivery("task-3"), 4)
            .await
            .expect_err("no live incarnation"),
        ContinuationSubmitError::OwnerRetired
    );
    assert_eq!(
        resolver
            .resolve_address(&LogicalRuntimeId::new(second.address))
            .await
            .expect("resolve"),
        AddressResolution::Retired
    );
}

/// A mob the host does not manage now may be registered later (a host that
/// restores mob handles after building its mob state): its members' rows
/// wait, never strand.
#[tokio::test]
async fn a_member_address_of_an_unregistered_mob_is_not_served() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let resolver = MobContinuationResolver::new(Arc::new(OneMob(handle)));
    let address = meerkat::member_delivery_address("no-such-mob", "w", 1).expect("member address");
    assert_eq!(
        resolver.resolve_address(&address).await.expect("resolve"),
        AddressResolution::NotServed
    );
}
