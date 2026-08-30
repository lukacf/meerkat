//! Controller-side routing for source-owned forked-participant capabilities
//! (issue #159, phase 2).
//!
//! The source-owner service and the member-host serving arms are covered by
//! `forked_participant_capabilities.rs` and the host batteries. THIS file pins
//! the controller's routing decisions and nothing else:
//!
//!   * creation routes by CURRENT source residency (local service vs. the V6
//!     `CreateForkedParticipant` command on the owning host), and
//!   * revocation routes by the CAPABILITY's own immutable owner route, so it
//!     still works after the source member is gone, and
//!   * explicit attachment/release route only to the local source owner; a host
//!     route is typed unsupported until coupled V6 Materialize/Release exists.
//!
//! Local rows run against a real controlling mob with a real local member and
//! a real durable capability store. Placed rows run against the scripted host
//! peer, which serves the V6 arms with real replay/dedup semantics and a
//! tamper hook, so the controller's strict wire→domain conversion is exercised
//! on values it did not mint.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_mob::ControlScope;
use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantRef, ForkedParticipantReleaseOutcome, ForkedParticipantRequestId,
    ForkedParticipantReusePolicy, ForkedParticipantRevocationOutcome,
};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeForkedParticipantOwnerRoute, BridgeProtocolVersion, BridgeRejectionCause,
};
use meerkat_mob::{
    AgentIdentity, ForkedParticipantLeaseOperation, ForkedParticipantOwnerHostRejection,
    ForkedParticipantSourceRejection, MobControlPrincipal, MobError,
};
use support::{
    ControllingMob, REAL_COMMS_TEST_LOCK, ScriptedForkedParticipantTamper, StallGate,
    control_principal, create_controlling_mob, create_controlling_mob_with_builder,
    member_identity_of, scripted_member_client_stalling, spawn_peer_comms_endpoint,
    spawn_scripted_host_peer, spawn_scripted_member_turn_responder, wait_until,
};

const TTL: Duration = Duration::from_secs(600);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn request_id(name: &str) -> ForkedParticipantRequestId {
    ForkedParticipantRequestId::new(name).expect("canonical request id")
}

async fn spawn_local_worker(controlling: &ControllingMob, member: &str) {
    controlling
        .handle
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::new("worker", member)
                .with_backend(meerkat_mob::MobBackendKind::Session),
        )
        .await
        .unwrap_or_else(|error| panic!("spawn local worker {member}: {error}"));
}

async fn create(
    controlling: &ControllingMob,
    member: &str,
    request: &str,
) -> Result<ForkedParticipantRef, MobError> {
    create_with(
        controlling,
        member,
        request,
        ForkedParticipantOperationScope::InvokeAndObserve,
        ForkedParticipantReusePolicy::OneShot,
        TTL,
    )
    .await
}

async fn create_with(
    controlling: &ControllingMob,
    member: &str,
    request: &str,
    scope: ForkedParticipantOperationScope,
    reuse: ForkedParticipantReusePolicy,
    ttl: Duration,
) -> Result<ForkedParticipantRef, MobError> {
    controlling
        .handle
        .create_forked_participant(
            MobControlPrincipal::Owner,
            identity(member),
            request_id(request),
            None,
            scope,
            reuse,
            ttl,
        )
        .await
}

fn attachment(id: &str) -> ForkedParticipantAttachmentId {
    ForkedParticipantAttachmentId::new(id).expect("canonical attachment id")
}

fn tampered(
    capability: &ForkedParticipantRef,
    field: &str,
    value: serde_json::Value,
) -> ForkedParticipantRef {
    let mut encoded = serde_json::to_value(capability).expect("serialize capability");
    encoded
        .as_object_mut()
        .expect("capability serializes as an object")
        .insert(field.to_string(), value);
    serde_json::from_value(encoded).expect("well-typed tampered reference")
}

fn source_rejection(error: &MobError) -> ForkedParticipantSourceRejection {
    match error {
        MobError::ForkedParticipantSourceIneligible { rejection, .. } => *rejection,
        other => panic!("expected a typed source-ineligibility, got {other:?}"),
    }
}

// ===========================================================================
// LOCAL source: this runtime's own source-owner service
// ===========================================================================

/// The local lane is genuinely source-owned: the minted route names the mob's
/// own realm, the provenance names the member's exact bridge session, and the
/// SAME `request_id` replays onto the identical capability instead of taking a
/// second fork.
#[tokio::test(flavor = "multi_thread")]
async fn local_create_is_source_owned_and_replays_the_exact_request() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-create").await;
    spawn_local_worker(&controlling, "researcher").await;
    let session_id = controlling.member_session_id(&identity("researcher")).await;

    let capability = create(&controlling, "researcher", "req-local-1")
        .await
        .expect("local create is served by the source-owner service");

    assert_eq!(capability.source_identity(), &identity("researcher"));
    assert!(
        matches!(
            capability.owner_route(),
            ForkedParticipantOwnerRoute::Local { .. }
        ),
        "a local source must mint a LOCAL owner route, got {:?}",
        capability.owner_route()
    );
    assert_eq!(
        capability.provenance().source_session_id,
        session_id,
        "provenance must name the member's exact current bridge session"
    );
    assert_ne!(
        capability.fork_session_id(),
        &session_id,
        "the branch is a distinct durable session, never the source itself"
    );

    let replayed = create(&controlling, "researcher", "req-local-1")
        .await
        .expect("the exact same request replays");
    assert_eq!(
        replayed, capability,
        "an exact replay converges on the identical capability"
    );

    // Durable truth, not just the returned value: one record, one fork.
    let record = controlling
        .storage_forked_participants
        .load_by_request_id(&request_id("req-local-1"))
        .await
        .expect("capability store read")
        .expect("the replayed request has exactly one durable record");
    assert_eq!(
        record.sidecar.capability_ref.as_ref(),
        Some(&capability),
        "the durable record holds the same reference the caller received"
    );
}

/// An identity the roster never seated answers the ordinary missing-member
/// error rather than reaching the capability service.
#[tokio::test(flavor = "multi_thread")]
async fn create_refuses_an_unknown_source_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-unknown-source").await;
    let error = create(&controlling, "ghost", "req-ghost")
        .await
        .expect_err("an unseated identity cannot be a fork source");
    assert!(
        matches!(error, MobError::MemberNotFound(ref member) if member == &identity("ghost")),
        "expected MemberNotFound, got {error:?}"
    );
}

/// A fork is only defined at a complete boundary, so a source with a run in
/// progress is refused as busy — before anything is reserved.
#[tokio::test(flavor = "multi_thread")]
async fn local_create_refuses_a_running_source() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let gate = StallGate::new();
    let controlling = create_controlling_mob_with_builder("fp-local-busy", {
        let gate = gate.clone();
        move |builder| builder.with_default_llm_client(scripted_member_client_stalling(gate))
    })
    .await;
    spawn_local_worker(&controlling, "researcher").await;

    let member = controlling
        .handle
        .member(&identity("researcher"))
        .await
        .expect("member handle");
    member
        .send("park this turn", HandlingMode::Queue)
        .await
        .expect("member send admits");

    // The parked stream holds the member's runtime in Running; poll until the
    // capability lane observes it rather than racing the admission.
    wait_until("the local source is observed as busy", || {
        let controlling = &controlling;
        async move {
            match create(controlling, "researcher", "req-busy").await {
                Err(error @ MobError::ForkedParticipantSourceIneligible { .. }) => {
                    source_rejection(&error) == ForkedParticipantSourceRejection::SourceBusy
                }
                _ => false,
            }
        }
    })
    .await;

    // Nothing was reserved for the refused request.
    assert!(
        controlling
            .storage_forked_participants
            .load_by_request_id(&request_id("req-busy"))
            .await
            .expect("capability store read")
            .is_none(),
        "a routing refusal must never reserve a durable capability identity"
    );
    gate.release();
}

/// Revocation routes by the capability's own owner route, so it survives the
/// source member's retirement: the durable record, not the roster, is the
/// authority.
#[tokio::test(flavor = "multi_thread")]
async fn local_revoke_succeeds_after_the_source_member_retires() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-revoke").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-local-revoke")
        .await
        .expect("local create");

    controlling
        .handle
        .retire(identity("researcher"))
        .await
        .expect("retire the source member");
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == identity("researcher")),
        "the source member is gone from the roster"
    );
    assert!(
        matches!(
            create(&controlling, "researcher", "req-after-retire").await,
            Err(MobError::MemberNotFound(_))
        ),
        "creating a NEW capability from the retired source is refused — only \
         revocation of the already-minted one still routes"
    );

    let outcome = controlling
        .handle
        .revoke_forked_participant(MobControlPrincipal::Owner, &capability)
        .await
        .expect("revocation routes by the capability's own owner route");
    assert!(
        matches!(outcome, ForkedParticipantRevocationOutcome::Revoked { .. }),
        "expected a terminal revocation, got {outcome:?}"
    );

    let repeat = controlling
        .handle
        .revoke_forked_participant(MobControlPrincipal::Owner, &capability)
        .await
        .expect("a repeated revocation converges");
    assert_eq!(repeat, ForkedParticipantRevocationOutcome::Converged);
}

/// Local attachment is source-owner service state, so attachment/release never
/// need a current roster entry. Replays preserve use count, while a distinct
/// concurrent attachment receives the lifecycle machine's typed Busy denial.
#[tokio::test(flavor = "multi_thread")]
async fn local_attach_release_routes_by_capability_and_replays() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-attach").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create_with(
        &controlling,
        "researcher",
        "req-local-attach",
        ForkedParticipantOperationScope::Observe,
        ForkedParticipantReusePolicy::OneShot,
        TTL,
    )
    .await
    .expect("local create");
    let attachment_a = attachment("local-a");

    let grant = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment_a.clone(),
        )
        .await
        .expect("local attach");
    assert_eq!(grant.attachment_id, attachment_a);
    assert_eq!(grant.use_index, 1);
    assert_eq!(grant.remaining_uses, 0);
    assert_eq!(grant.scope, ForkedParticipantOperationScope::Observe);
    assert!(
        !grant.replayed,
        "the first attach alone consumes the one-shot use"
    );

    let replay = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment_a.clone(),
        )
        .await
        .expect("exact attachment replay");
    assert!(replay.replayed);
    assert_eq!(replay.use_index, 1);

    let busy = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("local-b"),
        )
        .await
        .expect_err("a different attachment cannot overlap the active lease");
    assert!(matches!(busy, MobError::ForkedParticipantRefused(_)));

    let released = controlling
        .handle
        .release_forked_participant(MobControlPrincipal::Owner, &capability, &attachment_a)
        .await
        .expect("release one-shot attachment");
    assert_eq!(released, ForkedParticipantReleaseOutcome::Exhausted);
    let release_replay = controlling
        .handle
        .release_forked_participant(MobControlPrincipal::Owner, &capability, &attachment_a)
        .await
        .expect("release replay");
    assert_eq!(release_replay, ForkedParticipantReleaseOutcome::Replayed);
}

/// Explicit leases enter the same actor admission gate as the surrounding mob:
/// attachment needs SendCommand; release needs Cancel.
#[tokio::test(flavor = "multi_thread")]
async fn local_attachment_and_release_use_their_control_scopes() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-lease-scopes").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-local-lease-scopes")
        .await
        .expect("local create");
    let attachment = attachment("scoped");

    controlling
        .grant("lease-sender", &[ControlScope::SendCommand], None)
        .await;
    let sender = controlling.handle_as("lease-sender");
    sender
        .attach_forked_participant(
            control_principal("lease-sender"),
            &capability,
            attachment.clone(),
        )
        .await
        .expect("SendCommand grants attachment");
    assert!(matches!(
        sender
            .release_forked_participant(
                control_principal("lease-sender"),
                &capability,
                &attachment,
            )
            .await,
        Err(MobError::ScopeDenied(denial)) if denial.required == ControlScope::Cancel
    ));

    controlling
        .grant("lease-canceller", &[ControlScope::Cancel], None)
        .await;
    controlling
        .handle_as("lease-canceller")
        .release_forked_participant(
            control_principal("lease-canceller"),
            &capability,
            &attachment,
        )
        .await
        .expect("Cancel grants release");
}

/// Bounded use records attachment identity permanently, even after release:
/// A may not be reused to consume a third grant. Pending revocation
/// terminalizes only after the active attachment is explicitly released.
#[tokio::test(flavor = "multi_thread")]
async fn local_attachment_lifecycle_preserves_reuse_and_terminal_release() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-attachment-lifecycle").await;
    spawn_local_worker(&controlling, "researcher").await;

    let bounded = create_with(
        &controlling,
        "researcher",
        "req-local-bounded",
        ForkedParticipantOperationScope::InvokeAndObserve,
        ForkedParticipantReusePolicy::BoundedReuse { max_uses: 2 },
        TTL,
    )
    .await
    .expect("create bounded capability");
    let attachment_a = attachment("bounded-a");
    let attachment_b = attachment("bounded-b");
    controlling
        .handle
        .attach_forked_participant(MobControlPrincipal::Owner, &bounded, attachment_a.clone())
        .await
        .expect("attach A");
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(MobControlPrincipal::Owner, &bounded, &attachment_a)
            .await
            .expect("release A"),
        ForkedParticipantReleaseOutcome::Reusable
    );
    controlling
        .handle
        .attach_forked_participant(MobControlPrincipal::Owner, &bounded, attachment_b.clone())
        .await
        .expect("attach B");
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(MobControlPrincipal::Owner, &bounded, &attachment_b)
            .await
            .expect("release B"),
        ForkedParticipantReleaseOutcome::Exhausted
    );
    assert!(matches!(
        controlling
            .handle
            .attach_forked_participant(MobControlPrincipal::Owner, &bounded, attachment_a)
            .await,
        Err(MobError::ForkedParticipantRefused(_))
    ));

    let revoking = create_with(
        &controlling,
        "researcher",
        "req-local-revoking",
        ForkedParticipantOperationScope::Invoke,
        ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 },
        TTL,
    )
    .await
    .expect("create revoking capability");
    let revoking_attachment = attachment("revoking-a");
    controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &revoking,
            revoking_attachment.clone(),
        )
        .await
        .expect("attach revoking capability");
    assert_eq!(
        controlling
            .handle
            .revoke_forked_participant(MobControlPrincipal::Owner, &revoking)
            .await
            .expect("revoke while attached"),
        ForkedParticipantRevocationOutcome::PendingAttachedRelease
    );
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &revoking,
                &revoking_attachment,
            )
            .await
            .expect("release pending revocation"),
        ForkedParticipantReleaseOutcome::Revoked
    );
}

/// Host attachment is intentionally not a standalone bridge verb. Full
/// references continue to be proven by the source-owner store after retirement.
#[tokio::test(flavor = "multi_thread")]
async fn local_attachment_refuses_host_and_tampered_references_after_retirement() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-local-attachment-provenance").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-local-provenance")
        .await
        .expect("local create");
    let host_route = tampered(
        &capability,
        "owner_route",
        serde_json::json!({"kind": "host", "realm_id": "global", "host_id": "host-x"}),
    );
    let host_error = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &host_route,
            attachment("host-route"),
        )
        .await
        .expect_err("a host route cannot use the local explicit-lease API");
    assert!(matches!(
        host_error,
        MobError::ForkedParticipantRemoteLeaseUnsupported {
            operation: ForkedParticipantLeaseOperation::Attach
        }
    ));
    let host_release_error = controlling
        .handle
        .release_forked_participant(
            MobControlPrincipal::Owner,
            &host_route,
            &attachment("host-route"),
        )
        .await
        .expect_err("host release must also remain coupled to V6 Materialize/Release");
    assert!(matches!(
        host_release_error,
        MobError::ForkedParticipantRemoteLeaseUnsupported {
            operation: ForkedParticipantLeaseOperation::Release
        }
    ));

    controlling
        .handle
        .retire(identity("researcher"))
        .await
        .expect("retire source member");
    let tampered = tampered(
        &capability,
        "source_identity",
        serde_json::json!("some-other-source"),
    );
    assert!(matches!(
        controlling
            .handle
            .attach_forked_participant(MobControlPrincipal::Owner, &tampered, attachment("bad"))
            .await,
        Err(MobError::ForkedParticipantRefused(_))
    ));

    let attachment = attachment("after-retirement");
    controlling
        .handle
        .attach_forked_participant(MobControlPrincipal::Owner, &capability, attachment.clone())
        .await
        .expect("capability record remains authority after source roster retirement");
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(MobControlPrincipal::Owner, &capability, &attachment)
            .await
            .expect("release after retirement"),
        ForkedParticipantReleaseOutcome::Exhausted
    );
}

/// A fork session id is visible in a capability reference, but it is not
/// independent resume authority. Ordinary member resume must fail before it
/// can attach the protected session to a roster entry.
#[tokio::test(flavor = "multi_thread")]
async fn plain_resume_refuses_a_capability_owned_fork_session() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-protected-resume").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-protected-resume")
        .await
        .expect("local create");
    let spec = meerkat_mob::SpawnMemberSpec::new("worker", "bypass-attempt")
        .with_backend(meerkat_mob::MobBackendKind::Session)
        .with_resume_bridge_session_id(capability.fork_session_id().clone());
    assert!(matches!(
        controlling.handle.spawn_spec(spec).await,
        Err(MobError::ForkedParticipantResumeRequiresAttachment { session_id })
            if session_id == *capability.fork_session_id()
    ));
}

// ===========================================================================
// PLACED source: the V6 command family on the owning member host
// ===========================================================================

/// The placed lane carries the EXACT current source incarnation and replays
/// through the host's own request dedup: two identical requests mint exactly
/// one capability.
#[tokio::test(flavor = "multi_thread")]
async fn placed_create_carries_the_exact_incarnation_and_replays() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fp-placed-create-host").await;
    let controlling = create_controlling_mob("fp-placed-create").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("placed spawn commits from the scripted ack");

    let capability = create(&controlling, "remote", "req-placed-1")
        .await
        .expect("placed create routes over the supervisor bridge");

    let payloads = scripted.received_create_forked_participant_payloads();
    assert_eq!(payloads.len(), 1, "exactly one create crossed the bridge");
    let payload = &payloads[0];
    assert_eq!(payload.protocol_version, BridgeProtocolVersion::V6);
    assert_eq!(payload.request_id, "req-placed-1");
    assert_eq!(payload.source_member.agent_identity, "remote");
    assert_eq!(payload.source_member.host_id, report.host_id);
    assert_eq!(
        payload.source_member.binding_generation, payload.binding_generation,
        "the command's host binding generation and the source incarnation's must agree"
    );
    assert!(
        payload.source_member.generation > 0 || payload.source_member.fence_token > 0,
        "the create command carries a real residency fence, not a placeholder"
    );

    assert!(
        matches!(
            capability.owner_route(),
            ForkedParticipantOwnerRoute::Host { host_id, .. } if host_id.as_str() == report.host_id
        ),
        "the capability is owned by the addressed host, got {:?}",
        capability.owner_route()
    );
    assert_eq!(
        capability.provenance().source_session_id.to_string(),
        payload.source_member.member_session_id,
        "provenance echoes the exact source session the command named"
    );

    let replayed = create(&controlling, "remote", "req-placed-1")
        .await
        .expect("the exact same request replays");
    assert_eq!(replayed, capability);
    assert_eq!(
        scripted.minted_forked_participant_count(),
        1,
        "a replayed request must not mint a second capability"
    );
    assert_eq!(
        scripted.received_create_forked_participant_payloads().len(),
        2,
        "the replay is a real second round trip, converged by the owner"
    );
}

/// A host that never negotiated V6 is refused BEFORE any bridge traffic: the
/// negotiated protocol window is a machine fact, not something to discover by
/// sending an under-versioned command.
#[tokio::test(flavor = "multi_thread")]
async fn placed_create_requires_a_v6_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fp-placed-v5-host").await;
    scripted.advertise_protocol_versions(vec![
        BridgeProtocolVersion::V2,
        BridgeProtocolVersion::V3,
        BridgeProtocolVersion::V4,
        BridgeProtocolVersion::V5,
    ]);
    let controlling = create_controlling_mob("fp-placed-v5").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("a V5 host still materializes placed members");

    let error = create(&controlling, "remote", "req-placed-v5")
        .await
        .expect_err("a pre-V6 host cannot own forked participants");
    assert_eq!(
        source_rejection(&error),
        ForkedParticipantSourceRejection::HostProtocolUnsupported
    );
    assert!(
        scripted
            .received_create_forked_participant_payloads()
            .is_empty(),
        "an unsupported host must never receive the command"
    );
}

/// The owning host decides source freshness. Its typed stale-fence rejection
/// propagates to the caller as the bridge rejection it is, not as a local
/// guess or a silent retry.
#[tokio::test(flavor = "multi_thread")]
async fn placed_create_propagates_host_stale_source_fencing() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fp-placed-stale-host").await;
    let controlling = create_controlling_mob("fp-placed-stale").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("placed spawn commits");

    scripted.reject_next_create_forked_participant(
        BridgeRejectionCause::StaleFence,
        "source incarnation is stale",
    );
    let error = create(&controlling, "remote", "req-placed-stale")
        .await
        .expect_err("a stale source must not produce a capability");
    assert!(
        matches!(
            error,
            MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::StaleFence,
                ..
            }
        ),
        "expected the host's typed stale-fence rejection, got {error:?}"
    );
}

/// A capability the controller did not mint is validated, not trusted: every
/// tamper on the wire is refused rather than handed to a caller.
#[tokio::test(flavor = "multi_thread")]
async fn placed_create_rejects_tampered_capability_references() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fp-placed-tamper-host").await;
    let controlling = create_controlling_mob("fp-placed-tamper").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("placed spawn commits");

    for (index, tamper) in [
        ScriptedForkedParticipantTamper::MalformedCapabilityId,
        ScriptedForkedParticipantTamper::UnboundRevocationId,
        ScriptedForkedParticipantTamper::LocalOwnerRoute,
        ScriptedForkedParticipantTamper::ForeignOwnerHost,
        ScriptedForkedParticipantTamper::ForeignSourceIdentity,
    ]
    .into_iter()
    .enumerate()
    {
        scripted.tamper_next_created_forked_participant(tamper);
        let request = format!("req-tamper-{index}");
        let error = create(&controlling, "remote", &request)
            .await
            .expect_err("a tampered capability must never be returned");
        assert!(
            matches!(error, MobError::ForkedParticipantRefused(_)),
            "tamper {index} must be refused by conversion or validation, got {error:?}"
        );
    }

    // The untampered request still works, so the refusals above are the
    // tampering being caught rather than the lane being broken.
    scripted
        .received_create_forked_participant_payloads()
        .last()
        .expect("tamper rows really reached the host");
    create(&controlling, "remote", "req-tamper-clean")
        .await
        .expect("an untampered reply is still admitted");
}

/// Placed revocation routes by the capability's owner HOST and that host's
/// CURRENT bound authority — never by where the source member lives now. It
/// therefore still works once the source member is gone from the host.
#[tokio::test(flavor = "multi_thread")]
async fn placed_revoke_routes_by_capability_owner_host_after_the_source_is_absent() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("fp-placed-revoke-host").await;
    let controlling = create_controlling_mob("fp-placed-revoke").await;
    let report = controlling.bind_scripted(&scripted).await;
    // The scripted ack advertises a REAL probe endpoint as the member, so the
    // retirement ladder's member-directed lifecycle traffic has somewhere to
    // land. Nothing about the capability lane depends on it.
    let member = Arc::new(spawn_peer_comms_endpoint("fp-placed-revoke-member", true, None).await);
    scripted.script_member_identity("remote", member_identity_of(&member));
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("placed spawn commits");
    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("routable supervisor bridge peer");
    member.trust(supervisor).await;
    let capability = create(&controlling, "remote", "req-placed-revoke")
        .await
        .expect("placed create");

    let lifecycle_responder = spawn_scripted_member_turn_responder(Arc::clone(&member));
    controlling
        .handle
        .retire(identity("remote"))
        .await
        .expect("retire the placed source member");
    lifecycle_responder.shutdown_and_join().await;
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == identity("remote")),
        "the placed source member is gone from the roster"
    );

    let outcome = controlling
        .handle
        .revoke_forked_participant(MobControlPrincipal::Owner, &capability)
        .await
        .expect("revocation routes by the capability's owner host");
    assert!(
        matches!(outcome, ForkedParticipantRevocationOutcome::Revoked { .. }),
        "expected a terminal revocation, got {outcome:?}"
    );

    let payloads = scripted.received_revoke_forked_participant_payloads();
    let payload = payloads.last().expect("one revoke crossed the bridge");
    assert_eq!(payload.protocol_version, BridgeProtocolVersion::V6);
    assert_eq!(
        payload.source_member.host_id, report.host_id,
        "the revoke is addressed by the CAPABILITY's owner host"
    );
    assert!(
        matches!(
            &payload.capability.owner_route,
            BridgeForkedParticipantOwnerRoute::Host { host_id, .. } if host_id == &report.host_id
        ),
        "the exact wire reference is resent verbatim"
    );
    assert_eq!(
        payload.source_member.agent_identity,
        capability.source_identity().as_str(),
        "stable source identity provenance travels with the revoke"
    );
    assert_eq!(
        payload.source_member.member_session_id,
        capability.provenance().source_session_id.to_string(),
        "stable source session provenance travels with the revoke"
    );

    let repeat = controlling
        .handle
        .revoke_forked_participant(MobControlPrincipal::Owner, &capability)
        .await
        .expect("a repeated revocation converges");
    assert_eq!(repeat, ForkedParticipantRevocationOutcome::Converged);
}

/// Recovered revocation refuses persisted authority that cannot prove the
/// capability's exact owner route or V6 support. Neither refusal may send the
/// bearer capability to a host.
#[tokio::test(flavor = "multi_thread")]
async fn recovered_revoke_refuses_under_versioned_and_corrupt_owner_routes() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let source_host = spawn_scripted_host_peer("fp-recovered-source-host").await;
    let source = create_controlling_mob("fp-recovered-source").await;
    let source_report = source.bind_scripted(&source_host).await;
    source
        .spawn_placed("worker", "remote", &source_report.host_id)
        .await
        .expect("placed spawn commits");
    let capability = create(&source, "remote", "req-recovered-refusal")
        .await
        .expect("placed create");
    let transport = create_controlling_mob("fp-recovered-transport").await;

    let corrupt_route = tampered(
        &capability,
        "owner_route",
        serde_json::json!({
            "kind": "host",
            "realm_id": "global",
            "host_id": source_report.host_id,
        }),
    );
    source
        .handle
        .shutdown()
        .await
        .expect("source actor shuts down before persisted recovery");
    let corrupt_error = source
        .handle
        .revoke_forked_participant_with_recovery(
            MobControlPrincipal::Owner,
            Some(&transport.handle),
            &corrupt_route,
        )
        .await
        .expect_err("a foreign owner realm must fail closed");
    assert!(
        matches!(corrupt_error, MobError::ForkedParticipantRefused(_)),
        "the corrupt route is a typed capability refusal: {corrupt_error:?}"
    );
    assert!(
        source_host
            .received_revoke_forked_participant_payloads()
            .is_empty(),
        "route corruption must be rejected before bridge traffic"
    );
    let stopped_transport = create_controlling_mob("fp-recovered-stopped-transport").await;
    stopped_transport
        .handle
        .shutdown()
        .await
        .expect("recovery transport shuts down");
    let stopped_error = source
        .handle
        .revoke_forked_participant_with_recovery(
            MobControlPrincipal::Owner,
            Some(&stopped_transport.handle),
            &capability,
        )
        .await
        .expect_err("a stopped fallback transport must fail before sending");
    assert!(
        matches!(stopped_error, MobError::ActorCommandChannelClosed),
        "unexpected stopped-transport error: {stopped_error:?}"
    );
    assert!(
        source_host
            .received_revoke_forked_participant_payloads()
            .is_empty(),
        "a stopped fallback transport must not receive or send the capability"
    );
    let recovered = source
        .handle
        .revoke_forked_participant_with_recovery(
            MobControlPrincipal::Owner,
            Some(&transport.handle),
            &capability,
        )
        .await
        .expect("an intact V6 owner route recovers through a live transport");
    assert!(matches!(
        recovered,
        ForkedParticipantRevocationOutcome::Revoked { .. }
    ));
    assert_eq!(
        source_host
            .received_revoke_forked_participant_payloads()
            .len(),
        1,
        "recovery sends exactly one revoke under the persisted owner authority"
    );

    let legacy_host = spawn_scripted_host_peer("fp-recovered-v5-host").await;
    legacy_host.advertise_protocol_versions(vec![
        BridgeProtocolVersion::V2,
        BridgeProtocolVersion::V3,
        BridgeProtocolVersion::V4,
        BridgeProtocolVersion::V5,
    ]);
    let legacy_owner = create_controlling_mob("fp-recovered-v5-owner").await;
    let legacy_report = legacy_owner.bind_scripted(&legacy_host).await;
    let legacy_realm =
        meerkat_core::mob_realm_id(legacy_owner.mob_id.as_str()).expect("canonical mob realm");
    let under_versioned = tampered(
        &capability,
        "owner_route",
        serde_json::json!({
            "kind": "host",
            "realm_id": legacy_realm.as_str(),
            "host_id": legacy_report.host_id,
        }),
    );
    let live_error = legacy_owner
        .handle
        .revoke_forked_participant_with_recovery(
            MobControlPrincipal::Owner,
            Some(&transport.handle),
            &under_versioned,
        )
        .await
        .expect_err("a live source refusal must not use alternate authority");
    assert_eq!(
        source_rejection(&live_error),
        ForkedParticipantSourceRejection::HostProtocolUnsupported
    );
    assert!(
        legacy_host
            .received_revoke_forked_participant_payloads()
            .is_empty(),
        "a live source refusal must not trigger alternate bridge traffic"
    );
    legacy_owner
        .handle
        .shutdown()
        .await
        .expect("legacy owner actor shuts down before persisted recovery");
    let version_error = legacy_owner
        .handle
        .revoke_forked_participant_with_recovery(
            MobControlPrincipal::Owner,
            Some(&transport.handle),
            &under_versioned,
        )
        .await
        .expect_err("a pre-V6 persisted host binding must fail closed");
    assert!(matches!(
        version_error,
        MobError::ForkedParticipantOwnerHostUnavailable {
            rejection: ForkedParticipantOwnerHostRejection::ProtocolUnsupported,
            ..
        }
    ));
    assert!(
        legacy_host
            .received_revoke_forked_participant_payloads()
            .is_empty(),
        "a pre-V6 host must never receive the capability"
    );
}
