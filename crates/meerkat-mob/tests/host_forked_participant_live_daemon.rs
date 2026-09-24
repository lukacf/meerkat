//! Live host-daemon end-to-end for source-owned forked participants.
//!
//! Everything here runs against a REAL `MobHostActor` over REAL comms: a real
//! acceptor, real V6 bridge commands, a real persistent member session
//! service, a real durable `ForkedParticipantStore`, and the actor's own
//! periodic maintenance arm. The only concession to a test is the sweep
//! CADENCE, which is a composition input on `MobHostActorConfig` — the same
//! single `select!` arm runs either way, and no extra task or test-only loop
//! is introduced.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeForkedParticipantAttachment, BridgeForkedParticipantReuse, BridgeForkedParticipantScope,
    BridgeMemberIncarnation, BridgeProtocolVersion, MaterializeLaunchMode,
};
use meerkat_mob::forked_participant::{
    ForkedParticipantCapabilityId, ForkedParticipantOwnerRoute, ForkedParticipantService,
    bridge_ref, domain_ref,
};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeCreateForkedParticipantPayload, BridgeRejectionCause, BridgeReply,
};
use meerkat_mob::store::{ForkedParticipantStore, SqliteMobStores};
use support::{
    HostFixtureOptions, PeerCommsEndpoint, REAL_COMMS_TEST_LOCK, bind_then_materialize,
    sample_materialize_payload, sample_portable_member_spec, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, supervisor_spec_for_endpoint,
};

const MOB_ID: &str = "fp-live";
const SOURCE: &str = "researcher";
const BRANCH: &str = "branch-1";
const SWEEP: Duration = Duration::from_millis(120);
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

fn capability_realm() -> meerkat_core::RealmId {
    // Member sessions land in `mob.<mob_id>`; source ownership compares realms
    // exactly, so the composed capability service must own the same one.
    meerkat_core::mob_realm_id(MOB_ID).expect("mob realm")
}

/// Poll a durable condition instead of sleeping a fixed span: the actor's own
/// tick drives the work, so the test waits for the OUTCOME, never for a clock.
async fn until<F, Fut>(what: &str, mut probe: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if probe().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn create_payload(
    sender: &PeerCommsEndpoint,
    source: &BridgeMemberIncarnation,
    request_id: &str,
    ttl: Duration,
) -> BridgeCreateForkedParticipantPayload {
    BridgeCreateForkedParticipantPayload {
        supervisor: supervisor_spec_for_endpoint(sender, MOB_ID),
        epoch: 1,
        binding_generation: 1,
        protocol_version: BridgeProtocolVersion::V6,
        source_member: source.clone(),
        request_id: request_id.to_string(),
        prefix_message_count: None,
        scope: BridgeForkedParticipantScope::InvokeAndObserve,
        reuse: BridgeForkedParticipantReuse::OneShot,
        ttl_millis: u64::try_from(ttl.as_millis()).expect("ttl fits"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn live_daemon_capability_lifecycle_end_to_end() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let capability_dir = tempfile::tempdir().expect("capability store dir");
    let stores =
        SqliteMobStores::open(capability_dir.path().join("capabilities.db")).expect("open stores");
    let capability_store: Arc<dyn ForkedParticipantStore> =
        Arc::new(stores.forked_participant_store());

    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("fp-live-host")
            .with_member_build()
            .with_forked_participant_substrate(Arc::clone(&capability_store), capability_realm())
            .with_forked_participant_sweep_interval(SWEEP),
    )
    .await
    .expect("member-build fixture with a capability substrate");
    let sender = spawn_peer_comms_endpoint("fp-live-supervisor", true, None).await;
    sender.trust(fixture.host_peer_descriptor()).await;

    // ---- bind + materialize the SOURCE member over real comms ----
    let source = bind_then_materialize(&sender, &fixture, MOB_ID, SOURCE).await;
    // Build the source incarnation from DURABLE host truth rather than from
    // assumed tuple values: create routes by current residency, so the exact
    // recorded generation/fence/session is what the owner admits against.
    let source_row = fixture
        .host_binding_record(MOB_ID)
        .await
        .materialized
        .get(SOURCE)
        .cloned()
        .expect("the source member is materialized");
    assert_eq!(source_row.session_id, source.session_id);
    let source_incarnation = BridgeMemberIncarnation {
        mob_id: MOB_ID.to_string(),
        agent_identity: SOURCE.to_string(),
        host_id: fixture.host_peer_descriptor().peer_id.to_string(),
        binding_generation: 1,
        member_session_id: source_row.session_id.clone(),
        generation: source_row.generation,
        fence_token: source_row.fence_token,
    };

    // ---- CreateForkedParticipant over the wire ----
    // A fork is taken at a complete boundary, so the owner refuses a source
    // whose runtime is not idle. The freshly materialized member may still be
    // settling, so the exact same command is retried at the same request id —
    // which also exercises create idempotency on the live path.
    let create = BridgeCommand::CreateForkedParticipant(create_payload(
        &sender,
        &source_incarnation,
        "req-live-1",
        Duration::from_secs(3600),
    ));
    let mut created = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        let reply = sender
            .send_bridge_command_raw(&fixture.host_peer_descriptor(), &create, REPLY_TIMEOUT)
            .await
            .expect("create forked participant reply");
        match reply {
            BridgeReply::ForkedParticipantCreated(response) => {
                created = Some(response);
                break;
            }
            BridgeReply::Rejected {
                cause: BridgeRejectionCause::ForkedParticipantBusy,
                ..
            } => tokio::time::sleep(Duration::from_millis(50)).await,
            other => panic!("expected a created capability, got {other:?}"),
        }
    }
    let created = created.expect("the source settles and the capability is created");
    let capability = domain_ref(&created.capability).expect("created capability parses");
    let fork_session_id = capability.fork_session_id().to_string();
    assert!(
        matches!(
            capability.owner_route(),
            ForkedParticipantOwnerRoute::Host { .. }
        ),
        "a host-created capability is host-routed"
    );

    // ---- containment: a plain Resume of the fork session is refused ----
    let mut plain = sample_materialize_payload(
        &sender,
        1,
        sample_portable_member_spec(MOB_ID, BRANCH, "worker"),
        1,
        1,
        MaterializeLaunchMode::Resume {
            session_id: fork_session_id.clone(),
            resume_from_role: None,
        },
    );
    plain.protocol_version = BridgeProtocolVersion::V6;
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(plain.clone()),
            REPLY_TIMEOUT,
        )
        .await
        .expect("plain resume reply");
    let BridgeReply::Rejected { cause, .. } = reply else {
        panic!("a plain resume of a protected fork session must be refused, got {reply:?}");
    };
    assert_eq!(
        cause,
        BridgeRejectionCause::ForkedParticipantTampered,
        "the visible session id must not substitute for the bearer capability"
    );

    // ...and nothing was seated, so the capability is still attachable.
    let record = capability_store
        .load_exact(&capability)
        .await
        .expect("capability record");
    assert!(
        record.machine_state.active_attachment_id.is_none(),
        "a refused resume consumes no attachment"
    );

    // ---- capability-aware MaterializeMember: Resume + exact attachment ----
    let attachment = BridgeForkedParticipantAttachment {
        attachment_id: "attach-live-1".to_string(),
        capability: bridge_ref(&capability),
    };
    let mut attached = plain.clone();
    attached.forked_participant_attachment = Some(attachment.clone());
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(attached.clone()),
            REPLY_TIMEOUT,
        )
        .await
        .expect("attached materialize reply");
    let BridgeReply::MemberMaterialized(branch) = reply else {
        panic!("the exact capability must seat the branch, got {reply:?}");
    };
    assert_eq!(
        branch.session_id, fork_session_id,
        "the branch resumes its exact fork session"
    );

    // The durable host row carries the association, in the SAME record.
    let row = fixture
        .host_binding_record(MOB_ID)
        .await
        .materialized
        .get(BRANCH)
        .cloned()
        .expect("materialized branch row");
    let association = row
        .forked_participant_attachment
        .clone()
        .expect("the branch row carries its capability association");
    assert_eq!(association.capability, capability);
    assert_eq!(association.attachment_id.as_str(), "attach-live-1");

    // ---- exact replay: answered, with no second attach ----
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(attached.clone()),
            REPLY_TIMEOUT,
        )
        .await
        .expect("replay reply");
    let BridgeReply::MemberMaterialized(replayed) = reply else {
        panic!("an exact replay must be answered, got {reply:?}");
    };
    assert_eq!(replayed.session_id, fork_session_id);
    let record = capability_store
        .load_exact(&capability)
        .await
        .expect("capability record");
    assert_eq!(
        record.machine_state.use_count, 1,
        "an exact replay must not consume a second use"
    );

    // ---- restart over the persistent stores: containment survives ----
    let fixture = fixture.restart().await;
    let row = fixture
        .host_binding_record(MOB_ID)
        .await
        .materialized
        .get(BRANCH)
        .cloned()
        .expect("the branch row survives restart");
    assert_eq!(
        row.forked_participant_attachment.as_ref(),
        Some(&association),
        "the association survives restart byte-for-byte"
    );
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(plain.clone()),
            REPLY_TIMEOUT,
        )
        .await
        .expect("post-restart plain resume reply");
    assert!(
        matches!(
            reply,
            BridgeReply::Rejected { ref cause, .. }
                if *cause == BridgeRejectionCause::ForkedParticipantTampered
        ),
        "containment is durable, not process-local; got {reply:?}"
    );

    // ---- revoke, then let the coordinator DISAPPEAR ----
    // Revocation parks behind the live attachment. No ReleaseMember is ever
    // sent: only the host's own maintenance pass can finish this.
    let owner = ForkedParticipantService::new(
        capability.owner_route().clone(),
        Arc::clone(&capability_store),
        fixture
            .member_concrete_service
            .clone()
            .and_then(meerkat_mob::MobSessionService::forked_participant_source_runtime)
            .expect("the fixture's persistent service is a capability source runtime"),
    )
    .expect("owner-side service over the same durable store");
    let outcome = owner
        .revoke(capability.capability_id(), true)
        .await
        .expect("revoke");
    assert_eq!(
        outcome,
        meerkat_mob::forked_participant::ForkedParticipantRevocationOutcome::PendingAttachedRelease,
        "revocation parks behind the live attachment"
    );

    // ---- the LIVE actor's own periodic pass converges it ----
    let converge_store = Arc::clone(&capability_store);
    let converge_capability_id = capability.capability_id().clone();
    until(
        "the host autonomously disposes the branch and releases the attachment",
        || {
            let store = Arc::clone(&converge_store);
            let capability_id = converge_capability_id.clone();
            async move { capability_is_terminal_and_detached(store.as_ref(), &capability_id).await }
        },
    )
    .await;
    let cleanup_store = Arc::clone(&capability_store);
    let cleanup_capability_id = capability.capability_id().clone();
    until(
        "the host completes fork cleanup after releasing the attachment",
        || {
            let store = Arc::clone(&cleanup_store);
            let capability_id = cleanup_capability_id.clone();
            async move { capability_cleanup_is_complete(store.as_ref(), &capability_id).await }
        },
    )
    .await;

    let record = capability_store
        .load_by_capability_id(capability.capability_id())
        .await
        .expect("load")
        .expect("record");
    assert!(
        record.machine_state.active_attachment_id.is_none(),
        "the attachment was released by the host, with no coordinator involved"
    );

    // The member residency is gone from durable host truth, released not leaked.
    let binding = fixture.host_binding_record(MOB_ID).await;
    assert!(
        !binding.materialized.contains_key(BRANCH),
        "the branch residency is disposed"
    );
    assert!(
        binding.released.contains_key(BRANCH),
        "and its release receipt is durably recorded"
    );
    assert!(
        binding.forked_participant_obligations.is_empty(),
        "convergence leaves no unreconciled obligation"
    );

    // The fork body was archived by member disposal; the cleanup sweep must
    // still commit the capability machine's distinct completion boundary.
    use meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState;
    let record = capability_store
        .load_by_capability_id(capability.capability_id())
        .await
        .expect("load")
        .expect("record");
    assert_eq!(
        record.machine_state.cleanup_state,
        ForkedParticipantCleanupState::Complete,
        "an already-archived fork must converge cleanup instead of retaining debt: {:?}",
        record.cleanup_debt
    );
    assert!(
        !fixture
            .host_binding_record(MOB_ID)
            .await
            .materialized
            .values()
            .any(|row| row.session_id == fork_session_id),
        "no durable residency still claims the fork session"
    );

    fixture.shutdown().await;
}

/// The capability reached a terminal phase AND no attachment is held.
///
/// Fork-body cleanup is deliberately NOT part of the predicate: the branch was
/// seated as an ordinary member, so the member teardown archives that session
/// as part of disposal. The cleanup sweep then finds the body already gone.
/// What this test asserts is the part the host owns — terminalization and
/// attachment release without any coordinator — and the cleanup outcome is
/// asserted separately below so a change in that behaviour is visible rather
/// than silently absorbed.
async fn capability_is_terminal_and_detached(
    store: &dyn ForkedParticipantStore,
    capability_id: &ForkedParticipantCapabilityId,
) -> bool {
    use meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantLifecycleState;
    let Ok(Some(record)) = store.load_by_capability_id(capability_id).await else {
        return false;
    };
    matches!(
        record.machine_state.lifecycle_phase,
        ForkedParticipantLifecycleState::Revoked
            | ForkedParticipantLifecycleState::Expired
            | ForkedParticipantLifecycleState::Exhausted
    ) && record.machine_state.active_attachment_id.is_none()
}

async fn capability_cleanup_is_complete(
    store: &dyn ForkedParticipantStore,
    capability_id: &ForkedParticipantCapabilityId,
) -> bool {
    use meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState;

    store
        .load_by_capability_id(capability_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|record| {
            record.machine_state.cleanup_state == ForkedParticipantCleanupState::Complete
        })
}
