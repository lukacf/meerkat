//! #1497 member continuations resolve through the mob roster: an address names
//! one incarnation (identity at generation, in one incarnation of its mob),
//! served by its current bridge session, and retired by a respawn, a
//! retirement, or the destroy of its mob.

use super::*;
use crate::continuation::{MobContinuationResolver, MobHandleLookup};
use meerkat::{
    AddressObservation, AddressResolution, ContinuationAddressResolver, ContinuationDelivery,
    ContinuationHandling, ContinuationKey, ContinuationOwner, ContinuationOwnerService,
    ContinuationProducer, ContinuationResultRef, ContinuationStatus, ContinuationSubmitError,
    StrandedCause,
};
use meerkat_core::service::MobIncarnationId;
use meerkat_runtime::{InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryInbox};

struct OneMob(MobHandle);

#[async_trait::async_trait]
impl MobHandleLookup for OneMob {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle> {
        (self.0.mob_id() == mob_id).then(|| self.0.clone())
    }
}

/// The mob a host holds under one id, which the test replaces as a host's
/// state does when the mob is destroyed and created again under its id.
struct CurrentMob(std::sync::Mutex<MobHandle>);

impl CurrentMob {
    fn replace(&self, handle: MobHandle) {
        *self.0.lock().expect("current mob") = handle;
    }
}

#[async_trait::async_trait]
impl MobHandleLookup for CurrentMob {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle> {
        let handle = self.0.lock().expect("current mob").clone();
        (handle.mob_id() == mob_id).then_some(handle)
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
    let address = meerkat::member_delivery_address("no-such-mob", MobIncarnationId::mint(), "w", 1)
        .expect("member address");
    assert_eq!(
        resolver.resolve_address(&address).await.expect("resolve"),
        AddressResolution::NotServed
    );
}

/// A mob destroyed and created again under its id is a new mob incarnation
/// whose generations start over, so the new member under an old member's
/// identity is at the old member's generation. The address names the mob
/// incarnation: the old member's continuation strands as retired and is
/// never served by the new member, whose own continuations bind its own
/// address.
#[tokio::test]
async fn member_continuations_never_reach_a_mob_created_again_under_its_id() {
    let (first, _first_service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    first
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    let mobs = Arc::new(CurrentMob(std::sync::Mutex::new(first.clone())));
    let resolver = Arc::new(MobContinuationResolver::new(mobs.clone()));
    let continuations = ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        resolver.clone(),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );
    let owner = ContinuationOwner::Member {
        mob_id: first.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };
    let owed = first
        .submit_continuation(&continuations, &identity, delivery("task-1"), 1)
        .await
        .expect("submit to the first mob's member");
    let owed_address = LogicalRuntimeId::new(owed.address.clone());
    let generation = first
        .roster()
        .await
        .get_by_identity(&identity)
        .expect("member")
        .generation
        .get();

    first.destroy().await.expect("destroy the first mob");
    let (second, _second_service) = create_test_mob(sample_definition()).await;
    assert_eq!(second.mob_id(), first.mob_id());
    assert_ne!(
        second.incarnation(),
        first.incarnation(),
        "a create starts a new mob incarnation"
    );
    second
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn under the same identity");
    assert_eq!(
        second
            .roster()
            .await
            .get_by_identity(&identity)
            .expect("member")
            .generation
            .get(),
        generation,
        "the new mob's generations start over"
    );
    mobs.replace(second.clone());

    assert_eq!(
        resolver
            .resolve_address(&owed_address)
            .await
            .expect("resolve"),
        AddressResolution::Retired,
        "the destroyed mob's member is never served by the new one"
    );
    assert_eq!(
        resolver
            .observe_address(&owed_address)
            .await
            .expect("observe"),
        AddressObservation::Retired
    );
    assert_eq!(
        continuations
            .continuation_status(&owner, &ContinuationKey::new("task-1").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::Stranded {
            receipt: owed.clone(),
            cause: StrandedCause::OwnerRetired
        }
    );
    let fresh = second
        .submit_continuation(&continuations, &identity, delivery("task-2"), 2)
        .await
        .expect("a new key binds the new mob's member");
    assert_ne!(fresh.address, owed.address);
    let session = second
        .resolve_bridge_session_id(&identity)
        .await
        .expect("bridge session");
    assert_eq!(
        resolver
            .resolve_address(&LogicalRuntimeId::new(fresh.address))
            .await
            .expect("resolve"),
        AddressResolution::Session(session)
    );
}

/// A fork_off or council completion that ends after its mob was destroyed
/// submits through a handle kept from that mob. The continuation names the
/// mob by id, so it must not bind the member of a mob created again under
/// the id: the handle of a destroyed mob submits nothing.
#[tokio::test]
async fn a_handle_of_a_destroyed_mob_submits_nothing_to_the_mob_created_again() {
    let (first, _first_service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    first
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    first.destroy().await.expect("destroy the first mob");
    let (second, _second_service) = create_test_mob(sample_definition()).await;
    second
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn under the same identity");
    let continuations = ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        Arc::new(MobContinuationResolver::new(Arc::new(OneMob(
            second.clone(),
        )))),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );

    assert_eq!(
        first
            .submit_continuation(&continuations, &identity, delivery("late"), 1)
            .await
            .expect_err("the first mob is destroyed"),
        ContinuationSubmitError::OwnerRetired
    );
    let owner = ContinuationOwner::Member {
        mob_id: second.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };
    assert_eq!(
        continuations
            .continuation_status(&owner, &ContinuationKey::new("late").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::NotCommitted,
        "nothing was committed for the new mob's member"
    );
}

/// A handle kept from a mob whose actor has stopped (here a crash stop, so
/// its phase stays the last live one it published) must not bind the member
/// of a mob created again under the id: it names its own incarnation, and
/// the mob the host holds under the id is another.
#[tokio::test]
async fn a_stopped_mobs_handle_submits_nothing_to_the_mob_created_again() {
    let (first, _first_service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    first
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    first
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("stop the first mob's actor");
    assert!(
        matches!(
            first.status_observation_snapshot(),
            MobState::Creating | MobState::Running
        ),
        "a stopped actor keeps its last live phase"
    );
    let (second, _second_service) = create_test_mob(sample_definition()).await;
    assert_ne!(second.incarnation(), first.incarnation());
    second
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn under the same identity");
    let continuations = ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        Arc::new(MobContinuationResolver::new(Arc::new(OneMob(
            second.clone(),
        )))),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );

    let submitted = first
        .submit_continuation(&continuations, &identity, delivery("late"), 1)
        .await;
    let owner = ContinuationOwner::Member {
        mob_id: second.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };
    assert_eq!(
        continuations
            .continuation_status(&owner, &ContinuationKey::new("late").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::NotCommitted,
        "nothing is committed for the new mob's member: {submitted:?}"
    );
    assert_eq!(
        submitted.expect_err("the first mob's incarnation is not the one the host holds"),
        ContinuationSubmitError::OwnerRetired
    );
}

/// An address minted before 0.9.4 names no mob incarnation, and a mob
/// created since is a later incarnation under its id: it never serves the
/// rows owed to the members of an older mob.
#[tokio::test]
async fn an_address_without_a_mob_incarnation_is_retired_in_a_mob_created_since() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    let generation = handle
        .roster()
        .await
        .get_by_identity(&identity)
        .expect("member")
        .generation
        .get();
    let resolver = MobContinuationResolver::new(Arc::new(OneMob(handle.clone())));
    // Nothing mints the old form any more: spell it as a 0.9.3 store holds it.
    let escape = |component: &str| component.replace('%', "%25").replace(':', "%3A");
    let legacy = LogicalRuntimeId::new(format!(
        "rt:member:{}:{}:{generation}",
        escape(handle.mob_id().as_str()),
        escape(identity.as_str()),
    ));
    assert_eq!(
        legacy
            .member_address()
            .expect("an address from before 0.9.4")
            .map(|member| member.incarnation),
        Some(None),
        "it names no mob incarnation"
    );
    assert_eq!(
        resolver.resolve_address(&legacy).await.expect("resolve"),
        AddressResolution::Retired
    );
    assert_eq!(
        resolver.observe_address(&legacy).await.expect("observe"),
        AddressObservation::Retired
    );
}

/// The incarnation a mob from before 0.9.4 gets at its first resume serves
/// the addresses minted before (which name no mob incarnation) as well as
/// its own; no mob serves another incarnation's addresses.
#[tokio::test]
async fn only_a_migrated_mob_serves_addresses_without_a_mob_incarnation() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let directory = tempfile::tempdir().expect("temporary mob directory");
    let database = directory.path().join("mob.db");
    let created = MobBuilder::new(
        sample_definition(),
        MobStorage::persistent(&database).expect("open persistent mob storage"),
    )
    .with_session_service(service)
    .allow_ephemeral_sessions(true)
    .create()
    .await
    .expect("create mob");
    assert!(
        !created
            .serves_member_address_incarnation(None)
            .await
            .expect("read the creation record"),
        "a mob created since 0.9.4 serves no address from before"
    );
    assert!(
        created
            .serves_member_address_incarnation(Some(created.incarnation()))
            .await
            .expect("read")
    );
    created.shutdown().await.expect("stop the first runtime");
    drop(created);
    // The store as a 0.9.3 binary left it: the log, and no creation record.
    rusqlite::Connection::open(&database)
        .expect("open the store")
        .execute("DELETE FROM mob_runtime_incarnations", [])
        .expect("drop the creation record");

    let migrated = resume_persistent(&database).await.expect("first resume");
    assert!(
        migrated
            .serves_member_address_incarnation(None)
            .await
            .expect("read the creation record"),
        "the migrated incarnation serves the addresses its members had"
    );
    assert!(
        migrated
            .serves_member_address_incarnation(Some(migrated.incarnation()))
            .await
            .expect("read")
    );
    assert!(
        !migrated
            .serves_member_address_incarnation(Some(MobIncarnationId::mint()))
            .await
            .expect("read")
    );
    migrated.shutdown().await.expect("stop the resume");
}

/// A key a member's continuation bound in a mob that crash-stopped stays
/// bound when a mob is created again under the id: replaying it through the
/// new mob's member answers the first receipt, stranded (the first
/// incarnation never admitted it), and never commits it again for the new
/// incarnation; a new key binds the new incarnation. A keyed member send
/// relies on this replay: a retried send never admits twice, even across a
/// re-creation.
#[tokio::test]
async fn a_key_bound_before_a_crash_stop_replays_stranded_through_the_mob_created_again() {
    let (first, _first_service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("w-cont");
    first
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn");
    let current = Arc::new(CurrentMob(std::sync::Mutex::new(first.clone())));
    let continuations = ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        Arc::new(MobContinuationResolver::new(current.clone())),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );
    let owner = ContinuationOwner::Member {
        mob_id: first.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };
    let receipt = first
        .submit_continuation(&continuations, &identity, delivery("member_send:item-1"), 1)
        .await
        .expect("commit at the first incarnation");

    first
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("stop the first mob's actor");
    let (second, _second_service) = create_test_mob(sample_definition()).await;
    assert_ne!(second.incarnation(), first.incarnation());
    second
        .spawn_spec(SpawnMemberSpec::new("worker", identity.as_str()))
        .await
        .expect("spawn under the same identity");
    current.replace(second.clone());

    let replay = second
        .submit_continuation(&continuations, &identity, delivery("member_send:item-1"), 2)
        .await
        .expect("the key replays");
    assert_eq!(replay, receipt, "the first receipt, never a new commit");
    assert_eq!(
        continuations
            .continuation_status(
                &owner,
                &ContinuationKey::new("member_send:item-1").expect("key")
            )
            .await
            .expect("status"),
        ContinuationStatus::Stranded {
            receipt: receipt.clone(),
            cause: StrandedCause::OwnerRetired,
        },
        "the first incarnation never admitted it"
    );
    let fresh = second
        .submit_continuation(&continuations, &identity, delivery("member_send:item-2"), 3)
        .await
        .expect("a new key commits");
    assert_ne!(
        fresh.address, receipt.address,
        "a new key binds the new incarnation"
    );
}

/// A mob with the member `w-cont`, which the host's lookup holds.
async fn mob_with_a_member() -> (MobHandle, Arc<MockSessionService>, Arc<CurrentMob>) {
    let (first, first_service) = create_test_mob(sample_definition()).await;
    first
        .spawn_spec(SpawnMemberSpec::new("worker", "w-cont"))
        .await
        .expect("spawn");
    let mobs = Arc::new(CurrentMob(std::sync::Mutex::new(first.clone())));
    (first, first_service, mobs)
}

/// Stop `first`'s actor (a crash stop, so its phase stays live), then create
/// the mob again under its id, with the same member, and let the host's
/// lookup hold the new one.
async fn stop_and_create_again(
    first: &MobHandle,
    mobs: &CurrentMob,
) -> (MobHandle, Arc<MockSessionService>) {
    first
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("stop the first mob's actor");
    let (second, second_service) = create_test_mob(sample_definition()).await;
    assert_eq!(second.mob_id(), first.mob_id());
    assert_ne!(second.incarnation(), first.incarnation());
    second
        .spawn_spec(SpawnMemberSpec::new("worker", "w-cont"))
        .await
        .expect("spawn under the same identity");
    mobs.replace(second.clone());
    (second, second_service)
}

fn continuation_service(mobs: Arc<CurrentMob>) -> ContinuationOwnerService {
    ContinuationOwnerService::new(
        RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
        Arc::new(MobContinuationResolver::new(mobs)),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    )
}

/// The mob created again under the id reuses a key the stopped mob's member
/// bound: the key ledger is per member identity, so the live handle gets
/// today's replay of the first receipt, never `OwnerRetired`.
#[tokio::test]
async fn a_live_mob_reusing_a_key_an_earlier_incarnation_bound_replays_it() {
    let (first, _first_service, mobs) = mob_with_a_member().await;
    let identity = AgentIdentity::from("w-cont");
    let continuations = continuation_service(mobs.clone());
    let earlier = first
        .submit_continuation(&continuations, &identity, delivery("task-1"), 1)
        .await
        .expect("the first mob binds the key");
    let (second, _second_service) = stop_and_create_again(&first, &mobs).await;

    assert_eq!(
        second
            .submit_continuation(&continuations, &identity, delivery("task-1"), 2)
            .await
            .expect("the live mob's handle replays the key"),
        earlier
    );
}

/// A stopped mob's handle does not replay a key the mob created again under
/// the id bound: it would answer with the live incarnation's receipt.
#[tokio::test]
async fn a_stopped_mobs_handle_replays_nothing_the_mob_created_again_bound() {
    let (first, _first_service, mobs) = mob_with_a_member().await;
    let identity = AgentIdentity::from("w-cont");
    let (second, _second_service) = stop_and_create_again(&first, &mobs).await;
    let continuations = continuation_service(mobs);
    second
        .submit_continuation(&continuations, &identity, delivery("task-2"), 1)
        .await
        .expect("the live mob binds the key");

    assert_eq!(
        first
            .submit_continuation(&continuations, &identity, delivery("task-2"), 2)
            .await,
        Err(ContinuationSubmitError::OwnerRetired)
    );
}

/// The job outcomes a test owner holds: one fork_off job.
struct OneForkJob(meerkat::RetainedJobFacts);

#[async_trait::async_trait]
impl meerkat::RetainedJobSource for OneForkJob {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> meerkat::RetainedJobLookup {
        if producer == &ContinuationProducer::ForkOff && job_id == "job-1" {
            meerkat::RetainedJobLookup::Found(self.0.clone())
        } else {
            meerkat::RetainedJobLookup::Absent
        }
    }
}

/// A stopped mob's handle commits no retained job outcome for the mob the
/// host holds again under its id, as for a plain continuation.
#[tokio::test]
async fn a_stopped_mobs_handle_commits_no_retained_outcome_for_the_mob_created_again() {
    let (first, _first_service, mobs) = mob_with_a_member().await;
    let identity = AgentIdentity::from("w-cont");
    let (second, _second_service) = stop_and_create_again(&first, &mobs).await;
    let continuations = continuation_service(mobs);
    let jobs = OneForkJob(meerkat::RetainedJobFacts {
        owner_session_id: second
            .resolve_bridge_session_id(&identity)
            .await
            .expect("bridge session"),
        retained_work: None,
        result_digest: "sha256:result".into(),
    });
    let job = meerkat::RetainedJobRecord::from_owner(&jobs, ContinuationProducer::ForkOff, "job-1")
        .await
        .expect("read")
        .expect("committed job");
    let completion = ContinuationDelivery {
        key: ContinuationKey::new("fork_off:job-1").expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::ForkOff,
            producer_id: "job-1".into(),
            result_digest: "sha256:result".into(),
            summary: None,
        },
        body: "the fork finished".into(),
        handling: ContinuationHandling::Queue,
    };

    assert_eq!(
        first
            .submit_retained_continuation(&continuations, &identity, completion, &job, 1)
            .await,
        Err(ContinuationSubmitError::OwnerRetired)
    );
    let owner = ContinuationOwner::Member {
        mob_id: second.mob_id().as_str().to_string(),
        identity: identity.as_str().to_string(),
    };
    assert_eq!(
        continuations
            .continuation_status(
                &owner,
                &ContinuationKey::new("fork_off:job-1").expect("key")
            )
            .await
            .expect("status"),
        ContinuationStatus::NotCommitted
    );
}
