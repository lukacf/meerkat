//! Capability-aware attached spawn for HOST-owned forked participants
//! (issue #159, phase 3 — target-mob side, placed parity).
//!
//! `forked_participant_attached_spawn.rs` pins the LOCAL half of the same
//! public API. THIS file pins the HOST half: a capability whose owner route
//! names a bound member host seats through ORDINARY V6 placed materialization,
//! carrying its attachment on the `MaterializeMember` command the owning host
//! already serves.
//!
//! Two properties are the point of the whole design and are asserted directly:
//!
//!   * the controller admits NOTHING about the lease — it drives no local
//!     `attach`, writes no association row, and issues no separate release
//!     verb; the owning host is the single owner of that lifecycle truth, and
//!   * routing refusals are decided from recorded machine facts BEFORE any
//!     bridge traffic, so an unbound or pre-V6 host never learns a capability
//!     exists by receiving a command it cannot serve.
//!
//! Scripted-host rows pin the exact wire carrier and the retry/rejection
//! matrix. The closing row runs the whole path against a REAL host daemon with
//! a real durable capability store, including the containment rule that keeps
//! an ordinary placed Resume of a protected fork session out.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantRef, ForkedParticipantRequestId, ForkedParticipantReusePolicy, bridge_ref,
};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeMaterializePayload, BridgeProtocolVersion, BridgeRejectionCause, MaterializeLaunchMode,
};
use meerkat_mob::store::{ForkedParticipantStore, SqliteMobStores};
use meerkat_mob::{
    AgentIdentity, ForkedParticipantOwnerHostRejection, MobBackendKind, MobControlPrincipal,
    MobError, SpawnMemberSpec,
};
use support::{
    ControllingMob, HostFixtureOptions, REAL_COMMS_TEST_LOCK, ScriptedHostPeer,
    create_controlling_mob, member_identity_of, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, spawn_scripted_host_peer, spawn_scripted_member_turn_responder,
};

const TTL: Duration = Duration::from_secs(600);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn attachment(id: &str) -> ForkedParticipantAttachmentId {
    ForkedParticipantAttachmentId::new(id).expect("canonical attachment id")
}

async fn create(
    controlling: &ControllingMob,
    member: &str,
    request: &str,
) -> Result<ForkedParticipantRef, MobError> {
    controlling
        .handle
        .create_forked_participant(
            MobControlPrincipal::Owner,
            identity(member),
            ForkedParticipantRequestId::new(request).expect("canonical request id"),
            None,
            ForkedParticipantOperationScope::InvokeAndObserve,
            ForkedParticipantReusePolicy::OneShot,
            TTL,
        )
        .await
}

async fn seat(
    controlling: &ControllingMob,
    capability: &ForkedParticipantRef,
    attachment_id: &str,
    spec: SpawnMemberSpec,
) -> Result<meerkat_mob::runtime::AttachedForkedParticipantSpawn, MobError> {
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            capability,
            attachment(attachment_id),
            spec,
        )
        .await
}

/// Bind a scripted host, seat a placed source on it, and mint a HOST-owned
/// capability from that source.
async fn host_owned_capability(
    label: &str,
) -> (
    ScriptedHostPeer,
    ControllingMob,
    String,
    ForkedParticipantRef,
) {
    let scripted = spawn_scripted_host_peer(&format!("{label}-host")).await;
    let controlling = create_controlling_mob(label).await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "source", &report.host_id)
        .await
        .expect("placed source commits from the scripted ack");
    let capability = create(&controlling, "source", "req-host-1")
        .await
        .expect("placed create mints a host-owned capability");
    assert!(
        matches!(
            capability.owner_route(),
            ForkedParticipantOwnerRoute::Host { host_id, .. } if host_id.as_str() == report.host_id
        ),
        "the fixture capability is owned by the bound host"
    );
    (scripted, controlling, report.host_id, capability)
}

fn branch_payloads(scripted: &ScriptedHostPeer) -> Vec<BridgeMaterializePayload> {
    scripted
        .received_materialize_payloads()
        .into_iter()
        .filter(|payload| payload.spec.agent_identity == "branch")
        .collect()
}

/// Forge an owner route onto an existing reference. Used only to aim a
/// capability at a host whose ROUTING facts are the subject of the row; the
/// controller must refuse from machine state without contacting it.
fn rerouted(capability: &ForkedParticipantRef, host_id: &str) -> ForkedParticipantRef {
    let mut encoded = serde_json::to_value(capability).expect("serialize capability");
    encoded
        .as_object_mut()
        .expect("capability serializes as an object")
        .insert(
            "owner_route".to_string(),
            serde_json::json!({
                "kind": "host",
                "realm_id": capability.owner_route().realm_id().as_str(),
                "host_id": host_id,
            }),
        );
    serde_json::from_value(encoded).expect("well-typed rerouted reference")
}

// ===========================================================================
// The wire carrier: ordinary placed materialization, plus the attachment
// ===========================================================================

/// The same public API seats a HOST-owned capability, and the exact
/// attachment — full immutable reference plus attachment identity — appears on
/// the ordinary `MaterializeMember` command. The controller reports a
/// host-owned lease and keeps no association of its own.
#[tokio::test(flavor = "multi_thread")]
async fn host_attached_spawn_carries_the_exact_attachment_to_the_owning_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, controlling, host_id, capability) = host_owned_capability("fp-host-seat").await;

    // The ordinary placed spawn that seated the SOURCE carries no attachment:
    // the slot is not something every placed spawn fills in.
    assert!(
        scripted
            .received_materialize_payloads()
            .iter()
            .filter(|payload| payload.spec.agent_identity == "source")
            .all(|payload| payload.forked_participant_attachment.is_none()),
        "an ordinary placed spawn always sends None"
    );

    // No placement, no launch mode, no policy: the API owns all three.
    let seated = seat(
        &controlling,
        &capability,
        "att-1",
        SpawnMemberSpec::new("worker", "branch"),
    )
    .await
    .expect("a host-owned capability seats through ordinary placed materialization");

    assert_eq!(seated.spawn.agent_identity, identity("branch"));
    assert_eq!(seated.capability, capability);
    assert_eq!(seated.attachment_id, attachment("att-1"));
    assert_eq!(
        seated.lease.host_id().map(|host| host.as_str()),
        Some(host_id.as_str()),
        "the lease is reported as owned by the capability's own host"
    );
    assert!(
        seated.lease.grant().is_none(),
        "the controller never synthesizes a grant it did not observe"
    );

    let payloads = branch_payloads(&scripted);
    assert_eq!(payloads.len(), 1, "exactly one materialization was sent");
    let payload = &payloads[0];
    let carried = payload
        .forked_participant_attachment
        .as_ref()
        .expect("the branch materialization carries its attachment");
    assert_eq!(carried.attachment_id, "att-1");
    assert_eq!(
        carried.capability,
        bridge_ref(&capability),
        "the FULL immutable reference travels, field for field"
    );
    assert!(
        matches!(
            &payload.launch,
            MaterializeLaunchMode::Resume { session_id, resume_from_role: None }
                if session_id == &capability.fork_session_id().to_string()
        ),
        "the attachment resumes exactly its own fork session, got {:?}",
        payload.launch
    );
    // The association is spawn-request authority, never part of the member's
    // portable definition, so it must not reach the digested spec.
    let spec_json = serde_json::to_string(&payload.spec).expect("portable spec serializes");
    assert!(
        !spec_json.contains("att-1")
            && !spec_json.contains(capability.capability_id().expose_bearer_token()),
        "the attachment must not leak into PortableMemberSpec"
    );

    // Host-owned means host-owned: no controller-side association row exists.
    assert!(
        controlling
            .storage_metadata
            .list_forked_participant_member_associations(&controlling.mob_id)
            .await
            .expect("association listing")
            .is_empty(),
        "the controller retains no duplicate lifecycle truth for a host-owned lease"
    );
    scripted.shutdown();
}

/// A lost materialize reply is retried at the SAME idempotency tuple with the
/// SAME attachment, and the owning host replays its recorded ack instead of
/// admitting a second attachment.
#[tokio::test(flavor = "multi_thread")]
async fn host_attached_spawn_retry_replays_without_a_second_materialization() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, controlling, _host_id, capability) =
        host_owned_capability("fp-host-retry").await;

    // Served + recorded on the host, reply lost on the wire: the exact
    // resend-once class.
    scripted.drop_next_materialize_replies(1);
    seat(
        &controlling,
        &capability,
        "att-retry",
        SpawnMemberSpec::new("worker", "branch"),
    )
    .await
    .expect("the exactly-once resend converges on the recorded materialization");

    let payloads = branch_payloads(&scripted);
    assert_eq!(
        payloads.len(),
        2,
        "the lost reply is retried exactly once, never re-minted"
    );
    assert_eq!(
        payloads[0], payloads[1],
        "the retry is byte-identical: same generation, fence, spec digest, and attachment"
    );
    assert_eq!(
        scripted.materialize_dedup_rows_for("branch"),
        1,
        "the owning host recorded ONE materialization for the retried tuple"
    );
    let carried = payloads[1]
        .forked_participant_attachment
        .as_ref()
        .expect("the retry carries the same attachment");
    assert_eq!(carried.attachment_id, "att-retry");
    assert_eq!(carried.capability, bridge_ref(&capability));
    scripted.shutdown();
}

// ===========================================================================
// Routing refusals: decided from machine facts, before any traffic
// ===========================================================================

/// A capability aimed at a host this mob has no binding for is refused from
/// recorded machine state, and a bound host that never negotiated V6 is
/// refused the same way. Neither host receives a command.
#[tokio::test(flavor = "multi_thread")]
async fn host_attached_spawn_refuses_unbound_and_pre_v6_owner_hosts() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, controlling, _host_id, capability) =
        host_owned_capability("fp-host-routing").await;

    // A host this mob never bound.
    let unbound = rerouted(&capability, "never-bound-host");
    assert!(
        matches!(
            seat(
                &controlling,
                &unbound,
                "att-unbound",
                SpawnMemberSpec::new("worker", "branch"),
            )
            .await,
            Err(MobError::ForkedParticipantOwnerHostUnavailable {
                rejection: ForkedParticipantOwnerHostRejection::HostNotBound,
                ..
            })
        ),
        "an unbound owner host is refused before traffic"
    );

    // A bound host that stops below V6.
    let legacy = spawn_scripted_host_peer("fp-host-routing-v5").await;
    legacy.advertise_protocol_versions(vec![
        BridgeProtocolVersion::V2,
        BridgeProtocolVersion::V3,
        BridgeProtocolVersion::V4,
        BridgeProtocolVersion::V5,
    ]);
    let legacy_report = controlling.bind_scripted(&legacy).await;
    let pre_v6 = rerouted(&capability, &legacy_report.host_id);
    assert!(
        matches!(
            seat(
                &controlling,
                &pre_v6,
                "att-v5",
                SpawnMemberSpec::new("worker", "branch"),
            )
            .await,
            Err(MobError::ForkedParticipantOwnerHostUnavailable {
                rejection: ForkedParticipantOwnerHostRejection::ProtocolUnsupported,
                ..
            })
        ),
        "a bound pre-V6 host cannot own a capability seating"
    );
    assert!(
        legacy.received_materialize_payloads().is_empty(),
        "a pre-V6 host must never receive the materialization"
    );
    assert!(
        branch_payloads(&scripted).is_empty(),
        "no other host is contacted either"
    );
    legacy.shutdown();
    scripted.shutdown();
}

/// Placement, launch mode, and execution context are owned by the capability,
/// not the caller. Each conflicting declaration is refused before any traffic.
#[tokio::test(flavor = "multi_thread")]
async fn host_attached_spawn_refuses_conflicting_placement_session_and_policy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, controlling, host_id, capability) =
        host_owned_capability("fp-host-conflict").await;
    let other = spawn_scripted_host_peer("fp-host-conflict-other").await;
    let other_report = controlling.bind_scripted(&other).await;

    let rejected = |result: Result<_, MobError>, what: &str| match result {
        Err(MobError::ForkedParticipantAttachedSpawnSpecRejected { .. }) => {}
        other => panic!("{what} must be a typed spec rejection, got {other:?}"),
    };

    // Placement that names a different (bound, V6) host than the owner.
    rejected(
        seat(
            &controlling,
            &capability,
            "att-place",
            SpawnMemberSpec::new("worker", "branch").with_placement(
                meerkat_mob::machines::mob_machine::HostId(other_report.host_id.clone()),
            ),
        )
        .await,
        "a placement conflicting with the capability's owner host",
    );

    // Resume of a session that is not this capability's fork session.
    rejected(
        seat(
            &controlling,
            &capability,
            "att-session",
            SpawnMemberSpec::new("worker", "branch")
                .with_resume_bridge_session_id(meerkat_core::SessionId::new()),
        )
        .await,
        "a resume of the wrong session",
    );

    // A controller-local substrate for a host-owned capability.
    rejected(
        seat(
            &controlling,
            &capability,
            "att-backend",
            SpawnMemberSpec::new("worker", "branch").with_backend(MobBackendKind::Session),
        )
        .await,
        "a controller-local backend",
    );

    // An unmanaged external runtime owns neither the session nor a host.
    let mut external = SpawnMemberSpec::new("worker", "branch");
    external.binding = Some(meerkat_mob::RuntimeBinding::External {
        peer_id: "some-peer".to_string(),
        address: "tcp://127.0.0.1:1".to_string(),
        pubkey: [7u8; 32],
        bootstrap_token: None,
    });
    rejected(
        seat(&controlling, &capability, "att-external", external).await,
        "an unmanaged external binding",
    );

    // Policy widening stays refused on the host route exactly as on the local
    // one: the branch inherits the SOURCE's execution context.
    rejected(
        seat(
            &controlling,
            &capability,
            "att-policy",
            SpawnMemberSpec::new("worker", "branch").with_tool_access_policy(
                meerkat_core::ops::ToolAccessPolicy::DenyList(Default::default()),
            ),
        )
        .await,
        "a tool access policy override",
    );

    // The capability's own owner host may be restated, but only exactly.
    seat(
        &controlling,
        &capability,
        "att-exact",
        SpawnMemberSpec::new("worker", "branch")
            .with_placement(meerkat_mob::machines::mob_machine::HostId(host_id.clone())),
    )
    .await
    .expect("restating the capability's own owner host is admitted");

    assert_eq!(
        branch_payloads(&scripted).len(),
        1,
        "only the admitted seating reached the owning host"
    );
    assert!(
        other
            .received_materialize_payloads()
            .iter()
            .all(|payload| payload.spec.agent_identity != "branch"),
        "the conflicting host is never contacted for the branch"
    );
    other.shutdown();
    scripted.shutdown();
}

// ===========================================================================
// Teardown: the ordinary release path, and nothing extra
// ===========================================================================

/// Retiring the seated member emits the ORDINARY `ReleaseMember`, which is
/// what releases the host-owned association. The controller issues no second
/// capability command and holds no association to release.
#[tokio::test(flavor = "multi_thread")]
async fn host_attached_spawn_teardown_releases_through_ordinary_release_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let (scripted, controlling, _host_id, capability) =
        host_owned_capability("fp-host-teardown").await;
    // The scripted ack advertises a REAL probe endpoint as the branch member,
    // so the ordinary retirement ladder's member-directed lifecycle traffic
    // has somewhere to land. Nothing about the capability lane depends on it.
    let member = Arc::new(spawn_peer_comms_endpoint("fp-host-teardown-branch", true, None).await);
    scripted.script_member_identity("branch", member_identity_of(&member));
    seat(
        &controlling,
        &capability,
        "att-teardown",
        SpawnMemberSpec::new("worker", "branch"),
    )
    .await
    .expect("seat the branch");

    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("routable supervisor bridge peer");
    member.trust(supervisor).await;

    let before = scripted.received_revoke_forked_participant_payloads().len();
    let lifecycle_responder = spawn_scripted_member_turn_responder(Arc::clone(&member));
    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("ordinary retirement of the seated branch");
    lifecycle_responder.shutdown_and_join().await;

    let released = scripted.received_release_payloads();
    let branch_release = released
        .iter()
        .find(|payload| payload.agent_identity == "branch")
        .expect("retirement reached the owning host as ReleaseMember");
    let branch_materialize = &branch_payloads(&scripted)[0];
    assert_eq!(
        (branch_release.generation, branch_release.fence_token),
        (
            branch_materialize.generation,
            branch_materialize.fence_token
        ),
        "the release names the exact seated incarnation"
    );
    assert_eq!(
        scripted.received_revoke_forked_participant_payloads().len(),
        before,
        "teardown issues NO extra capability command; ReleaseMember is the path"
    );
    assert!(
        controlling
            .storage_metadata
            .list_forked_participant_member_associations(&controlling.mob_id)
            .await
            .expect("association listing")
            .is_empty(),
        "no controller-side association was ever created, so none is left behind"
    );
    scripted.shutdown();
}

// ===========================================================================
// Real host daemon: the whole path, including containment
// ===========================================================================

/// End to end against a REAL member host with a real durable capability store:
/// the controller mints a host-owned capability, an ordinary placed Resume of
/// the protected fork session is denied by the owning host, and the same
/// public API then seats the branch by carrying its attachment.
#[tokio::test(flavor = "multi_thread")]
async fn real_host_denies_plain_resume_and_admits_the_attached_spawn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-host-live").await;
    let realm = meerkat_core::mob_realm_id(controlling.mob_id.as_str()).expect("mob realm");
    let capability_dir = tempfile::tempdir().expect("capability store dir");
    let stores =
        SqliteMobStores::open(capability_dir.path().join("capabilities.db")).expect("open stores");
    let capability_store: Arc<dyn ForkedParticipantStore> =
        Arc::new(stores.forked_participant_store());
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named("fp-host-live-host")
            .with_member_build()
            .with_forked_participant_substrate(Arc::clone(&capability_store), realm),
    )
    .await
    .expect("member-build fixture with a capability substrate");
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "source", &report.host_id)
        .await
        .expect("placed source materializes on the real host");

    // A fork is only defined at a complete boundary, so the owner refuses a
    // source that is still settling. Retry the exact same request id, which
    // also exercises create idempotency on the live path.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let capability = loop {
        match create(&controlling, "source", "req-live-host").await {
            Ok(capability) => break capability,
            Err(MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::ForkedParticipantBusy,
                ..
            }) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("real host create: {error:?}"),
        }
    };
    assert!(matches!(
        capability.owner_route(),
        ForkedParticipantOwnerRoute::Host { .. }
    ));

    // Containment: an ordinary placed Resume that merely NAMES the protected
    // fork session proves nothing, and the owning host refuses it.
    let mut plain = support::placed_spawn_spec("worker", "plain", &report.host_id);
    plain = plain.with_resume_bridge_session_id(capability.fork_session_id().clone());
    let denied = controlling
        .handle
        .spawn_spec(plain)
        .await
        .expect_err("a plain placed resume of a protected fork session is denied");
    assert!(
        format!("{denied:?}").contains("ForkedParticipant"),
        "the denial names the capability rule, got {denied:?}"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .all(|entry| entry.agent_identity != identity("plain")),
        "a denied resume seats nothing"
    );

    // The same public API, carrying the attachment, is admitted.
    let seated = seat(
        &controlling,
        &capability,
        "att-live",
        SpawnMemberSpec::new("worker", "branch"),
    )
    .await
    .expect("the attached spawn is admitted by the real owning host");
    assert_eq!(seated.spawn.agent_identity, identity("branch"));
    assert_eq!(
        seated.lease.host_id().map(|host| host.as_str()),
        Some(report.host_id.as_str())
    );

    // The host recorded the association against the seated member row; the
    // controller recorded nothing.
    let row = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("branch")
        .cloned()
        .expect("the branch is materialized on the real host");
    assert_eq!(row.session_id, capability.fork_session_id().to_string());
    assert!(
        controlling
            .storage_metadata
            .list_forked_participant_member_associations(&controlling.mob_id)
            .await
            .expect("association listing")
            .is_empty(),
        "the owning host is the single owner of the association"
    );

    // Ordinary retirement reaches the host and clears the row.
    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("ordinary retirement releases on the owning host");
    let record = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await;
    assert!(
        !record.materialized.contains_key("branch"),
        "ReleaseMember cleared the seated row"
    );
    fixture.shutdown().await;
}
