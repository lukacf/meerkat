//! Capability-aware attached spawn for LOCAL source-owned forked participants
//! (issue #159, phase 3 — target-mob side).
//!
//! `forked_participant_routing.rs` pins WHICH owner serves a capability verb.
//! THIS file pins what happens when a LOCAL capability is seated as an
//! ordinary target-mob member:
//!
//!   * a plain resume of a protected fork session stays refused, while an
//!     unprotected session resumes unchanged, and
//!   * the capability-aware spawn seats the branch through the ORDINARY Resume
//!     pipeline without bypassing or leaking its lease, and
//!   * durable association/obligation custody survives every failure shape and
//!     a process restart, and
//!   * teardown releases the exact attachment exactly once, after the member's
//!     session/runtime teardown.
//!
//! Every row runs against a real mob actor, a real durable capability store, a
//! real lifecycle machine, and the real persistent session service. Nothing
//! here stubs the seam it is testing.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantRef, ForkedParticipantReleaseOutcome, ForkedParticipantRequestId,
    ForkedParticipantReusePolicy,
};
use meerkat_mob::{
    AgentIdentity, ForkedParticipantOwnerHostRejection, MobBackendKind, MobControlPrincipal,
    MobError, SpawnMemberSpec,
};
use support::{ControllingMob, REAL_COMMS_TEST_LOCK, create_controlling_mob, wait_until};

const TTL: Duration = Duration::from_secs(600);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn attachment(id: &str) -> ForkedParticipantAttachmentId {
    ForkedParticipantAttachmentId::new(id).expect("canonical attachment id")
}

async fn spawn_local_worker(controlling: &ControllingMob, member: &str) {
    controlling
        .handle
        .spawn_spec(SpawnMemberSpec::new("worker", member).with_backend(MobBackendKind::Session))
        .await
        .unwrap_or_else(|error| panic!("spawn local worker {member}: {error}"));
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

/// A LOCAL seating must carry the owner's real typed grant; a host-owned
/// lease deliberately carries none, so reading one here is itself an assertion
/// that this row took the local route.
fn local_grant(
    seated: &meerkat_mob::runtime::AttachedForkedParticipantSpawn,
) -> &meerkat_mob::forked_participant::ForkedParticipantGrant {
    seated
        .lease
        .grant()
        .expect("a LOCAL attached spawn reports the source owner's typed grant")
}

fn branch_spec(member: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new("worker", member).with_backend(MobBackendKind::Session)
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

async fn associations(
    controlling: &ControllingMob,
) -> Vec<meerkat_mob::store::MobForkedParticipantMemberAssociation> {
    controlling
        .storage_metadata
        .list_forked_participant_member_associations(&controlling.mob_id)
        .await
        .expect("durable association listing")
}

// ===========================================================================
// Containment: the plain resume guard, and what it deliberately does NOT touch
// ===========================================================================

/// The guard is exact: only a session the capability store owns is protected.
/// An ordinary member session resumes through the unchanged plain path.
#[tokio::test(flavor = "multi_thread")]
async fn plain_resume_is_refused_only_for_protected_fork_sessions() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-guard").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-guard")
        .await
        .expect("local create");

    let protected = branch_spec("bypass-attempt")
        .with_resume_bridge_session_id(capability.fork_session_id().clone());
    assert!(
        matches!(
            controlling.handle.spawn_spec(protected).await,
            Err(MobError::ForkedParticipantResumeRequiresAttachment { session_id })
                if session_id == *capability.fork_session_id()
        ),
        "a plain resume may not seat a capability-owned fork session"
    );

    // An UNPROTECTED session is untouched by the guard. Spawn a second
    // ordinary member, retire it, and resume its own (non-capability) session
    // through the unchanged plain path.
    spawn_local_worker(&controlling, "plain").await;
    let plain_session = controlling
        .handle
        .get_member(&identity("plain"))
        .await
        .expect("member read")
        .expect("plain member is seated")
        .bridge_session_id()
        .expect("local member has a bridge session")
        .clone();
    assert_ne!(
        plain_session,
        *capability.fork_session_id(),
        "the fork session is a distinct child, not an ordinary member session"
    );
    controlling
        .handle
        .retire(identity("plain"))
        .await
        .expect("retire the plain member");
    controlling
        .handle
        .spawn_spec(branch_spec("plain").with_resume_bridge_session_id(plain_session))
        .await
        .expect("an unprotected session still resumes through the plain path");
}

// ===========================================================================
// The happy path
// ===========================================================================

/// Seating succeeds from a source whose roster member is already GONE: the
/// capability record, not the roster, is authority. The seated member resumes
/// the capability's own fork session and the durable association records the
/// full reference, the attachment, and the seated incarnation.
#[tokio::test(flavor = "multi_thread")]
async fn attached_spawn_seats_a_branch_after_the_source_member_is_gone() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-seat").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-seat")
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
        "the source roster member is gone before the branch is seated"
    );

    let seated = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("seat-1"),
            branch_spec("branch"),
        )
        .await
        .expect("capability-aware attached spawn");

    assert_eq!(seated.spawn.agent_identity, identity("branch"));
    assert_eq!(seated.attachment_id, attachment("seat-1"));
    assert_eq!(seated.capability, capability);
    assert!(
        !local_grant(&seated).replayed,
        "the first seating consumes a real use of the capability"
    );
    assert_eq!(local_grant(&seated).use_index, 1);

    let member = controlling
        .handle
        .get_member(&identity("branch"))
        .await
        .expect("member read")
        .expect("the branch is an ordinary roster member");
    assert_eq!(
        member.bridge_session_id(),
        Some(capability.fork_session_id()),
        "the branch runs the capability's own fork session"
    );

    let recorded = associations(&controlling).await;
    assert_eq!(recorded.len(), 1, "exactly one association: {recorded:?}");
    let record = &recorded[0];
    assert_eq!(record.agent_identity, identity("branch"));
    assert_eq!(record.association.capability, capability);
    assert_eq!(record.association.attachment_id, attachment("seat-1"));
    assert!(
        record.obligation.is_none(),
        "a seated association owes no reconciliation: {record:?}"
    );
    let target = record
        .target
        .as_ref()
        .expect("a committed seating records its exact incarnation");
    assert_eq!(target.session_id, capability.fork_session_id().to_string());
    assert_eq!(target.generation, member.generation.get());
    assert_eq!(target.fence_token, member.fence_token.get());
}

/// An exact replay is idempotent on both halves: the machine replays the
/// grant rather than consuming a second use, and the retried spawn does not
/// mint a second association or a second member.
#[tokio::test(flavor = "multi_thread")]
async fn attached_spawn_replay_does_not_duplicate_the_attachment() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-replay").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-replay")
        .await
        .expect("local create");

    let first = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("replay-1"),
            branch_spec("branch"),
        )
        .await
        .expect("first seating");
    assert!(!local_grant(&first).replayed);

    // The retry re-presents the exact attachment. The lifecycle machine
    // replays the grant, and the seating converges on the member it already
    // seats instead of driving a second spawn.
    let retry = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("replay-1"),
            branch_spec("branch"),
        )
        .await
        .expect("an exact replay converges on the existing seating");
    assert!(
        local_grant(&retry).replayed,
        "the replayed attach must not consume a second use"
    );
    assert_eq!(local_grant(&retry).use_index, local_grant(&first).use_index);
    assert_eq!(retry.spawn.agent_identity, first.spawn.agent_identity);

    // A replayed attachment may not be re-aimed at a different member.
    let reaimed = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("replay-1"),
            branch_spec("other-branch"),
        )
        .await
        .expect_err("a bound attachment cannot seat a second member");
    assert!(matches!(
        reaimed,
        MobError::ForkedParticipantAttachedSpawnSpecRejected { .. }
    ));
    assert!(
        controlling
            .handle
            .get_member(&identity("other-branch"))
            .await
            .expect("member read")
            .is_none(),
        "the refused re-aim never seated a second member"
    );

    let recorded = associations(&controlling).await;
    assert_eq!(
        recorded.len(),
        1,
        "an exact replay never mints a second association: {recorded:?}"
    );
    assert_eq!(recorded[0].agent_identity, identity("branch"));

    // The capability's one-shot budget was not consumed twice: releasing the
    // still-held attachment exhausts it exactly once.
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &capability,
                &attachment("replay-1"),
            )
            .await
            .expect("release the seated attachment"),
        ForkedParticipantReleaseOutcome::Exhausted
    );
}

// ===========================================================================
// Failure shapes
// ===========================================================================

/// A definitively refused spawn releases the exact attachment it took, and
/// leaves no durable association behind.
#[tokio::test(flavor = "multi_thread")]
async fn definitive_spawn_failure_releases_the_exact_attachment() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-definitive").await;
    spawn_local_worker(&controlling, "researcher").await;
    spawn_local_worker(&controlling, "occupied").await;
    let capability = create(&controlling, "researcher", "req-definitive")
        .await
        .expect("local create");

    let refused = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("definitive"),
            branch_spec("occupied"),
        )
        .await
        .expect_err("seating onto an occupied identity is refused");
    assert!(
        matches!(refused, MobError::MemberAlreadyExists(_)),
        "expected a typed duplicate-member refusal, got {refused:?}"
    );

    assert!(
        associations(&controlling).await.is_empty(),
        "a proven compensating release discharges its durable custody"
    );
    // The attachment really was released: a fresh attachment can be taken.
    let regrant = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("after-release"),
        )
        .await;
    assert!(
        regrant.is_err(),
        "the one-shot budget was consumed by the released attachment, so a NEW \
         attachment is denied rather than silently granted: {regrant:?}"
    );
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &capability,
                &attachment("definitive"),
            )
            .await
            .expect("the exact released attachment replays"),
        ForkedParticipantReleaseOutcome::Replayed,
        "the compensating release already happened; a repeat replays instead of \
         releasing a second time"
    );
}

/// A capability whose branch is already seated under one member identity
/// cannot be re-seated under another. That failure is UNCLASSIFIED, so the
/// attachment is retained as an explicit durable obligation rather than
/// blind-released — the mob cannot prove the seating left nothing behind.
#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_spawn_failure_retains_a_durable_obligation() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-ambiguous").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = controlling
        .handle
        .create_forked_participant(
            MobControlPrincipal::Owner,
            identity("researcher"),
            ForkedParticipantRequestId::new("req-ambiguous").expect("canonical request id"),
            None,
            ForkedParticipantOperationScope::InvokeAndObserve,
            ForkedParticipantReusePolicy::BoundedReuse { max_uses: 2 },
            TTL,
        )
        .await
        .expect("local create");

    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("ambiguous-a"),
            branch_spec("branch-a"),
        )
        .await
        .expect("first seating");
    controlling
        .handle
        .retire(identity("branch-a"))
        .await
        .expect("retire the first seating, releasing its attachment");
    assert!(
        associations(&controlling).await.is_empty(),
        "the first seating's association was discharged by its teardown"
    );

    // The capability still has a use left, so the ATTACH is admitted. The
    // seating spawn then fails against the branch's already-established
    // durable identity.
    let failure = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("ambiguous-b"),
            branch_spec("branch-b"),
        )
        .await
        .expect_err("a branch cannot be re-seated under a second member identity");
    assert!(
        !matches!(failure, MobError::ForkedParticipantRefused(_)),
        "the failure must come from the spawn half, not the attach: {failure:?}"
    );

    let recorded = associations(&controlling).await;
    assert_eq!(
        recorded.len(),
        1,
        "an ambiguous failure retains custody: {recorded:?}"
    );
    assert_eq!(recorded[0].agent_identity, identity("branch-b"));
    let obligation = recorded[0]
        .obligation
        .expect("an ambiguous failure records an explicit obligation");
    assert!(
        matches!(
            obligation,
            meerkat_mob::store::MobForkedParticipantObligationCause::AmbiguousSpawn
                | meerkat_mob::store::MobForkedParticipantObligationCause::SpawnInFlight
        ),
        "expected a retained (never blind-released) obligation, got {obligation:?}"
    );
    assert!(
        recorded[0].target.is_none(),
        "nothing was seated, so no incarnation may be claimed"
    );

    // Never blind-released: the exact attachment is still ACTIVE on the
    // owner's lifecycle machine, so a different attachment is refused as busy.
    let busy = controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("competitor"),
        )
        .await
        .expect_err("the retained attachment still holds the lease");
    assert!(matches!(busy, MobError::ForkedParticipantRefused(_)));
}

// ===========================================================================
// Admission: routes, references, and policy widening
// ===========================================================================

/// A host-owned capability whose owner host is not bound in THIS mob is
/// refused from recorded machine facts, before any bridge traffic. The verb
/// itself now serves host routes (see `forked_participant_host_attached_spawn`
/// for the seating rows); what it will not do is contact — or invent — a host
/// this mob has no binding for.
#[tokio::test(flavor = "multi_thread")]
async fn attached_spawn_refuses_an_unbound_owner_host() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-host-route").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-host-route")
        .await
        .expect("local create");
    let host_routed = tampered(
        &capability,
        "owner_route",
        serde_json::json!({
            "kind": "host",
            "realm_id": capability.owner_route().realm_id().as_str(),
            "host_id": "remote-host",
        }),
    );
    assert!(matches!(
        host_routed.owner_route(),
        ForkedParticipantOwnerRoute::Host { .. }
    ));

    assert!(
        matches!(
            controlling
                .handle
                .spawn_attached_forked_participant(
                    MobControlPrincipal::Owner,
                    &host_routed,
                    attachment("host-route"),
                    branch_spec("branch"),
                )
                .await,
            Err(MobError::ForkedParticipantOwnerHostUnavailable {
                rejection: ForkedParticipantOwnerHostRejection::HostNotBound,
                ..
            })
        ),
        "an unbound owner host is a typed routing refusal, never local service \
         and never speculative bridge traffic"
    );
    assert!(
        associations(&controlling).await.is_empty(),
        "a refused route never takes custody"
    );
}

/// A tampered reference, a resume of the wrong session, and a policy override
/// are each refused BEFORE any lease is granted.
#[tokio::test(flavor = "multi_thread")]
async fn attached_spawn_refuses_tampered_wrong_session_and_widened_specs() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-admission").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = controlling
        .handle
        .create_forked_participant(
            MobControlPrincipal::Owner,
            identity("researcher"),
            ForkedParticipantRequestId::new("req-admission").expect("canonical request id"),
            None,
            ForkedParticipantOperationScope::Observe,
            ForkedParticipantReusePolicy::OneShot,
            TTL,
        )
        .await
        .expect("local create");
    let source_session = controlling
        .handle
        .get_member(&identity("researcher"))
        .await
        .expect("member read")
        .expect("source member is seated")
        .bridge_session_id()
        .expect("local member has a bridge session")
        .clone();

    // 1. A widened tool policy is refused: the branch INHERITS the source's
    //    execution context and the capability layer never replaces it.
    let mut widened = branch_spec("branch");
    widened.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::default());
    assert!(
        matches!(
            controlling
                .handle
                .spawn_attached_forked_participant(
                    MobControlPrincipal::Owner,
                    &capability,
                    attachment("widened"),
                    widened,
                )
                .await,
            Err(MobError::ForkedParticipantAttachedSpawnSpecRejected { .. })
        ),
        "a policy override is refused rather than silently ignored"
    );

    // 2. A resume that names a DIFFERENT session is a conflict, not an
    //    override: the API owns the launch mode.
    assert!(
        matches!(
            controlling
                .handle
                .spawn_attached_forked_participant(
                    MobControlPrincipal::Owner,
                    &capability,
                    attachment("wrong-session"),
                    branch_spec("branch").with_resume_bridge_session_id(source_session),
                )
                .await,
            Err(MobError::ForkedParticipantAttachedSpawnSpecRejected { .. })
        ),
        "the only legal launch is a resume of this capability's own fork session"
    );

    // 3. A tampered reference fails the store's FULL-reference comparison.
    let widened_scope = tampered(
        &capability,
        "scope",
        serde_json::json!("invoke_and_observe"),
    );
    let widened_reuse = tampered(
        &capability,
        "reuse",
        serde_json::json!({"kind": "bounded_reuse", "max_uses": 32}),
    );
    for (label, reference) in [("scope", &widened_scope), ("reuse", &widened_reuse)] {
        assert_ne!(
            reference, &capability,
            "the {label} tamper must actually differ from the minted reference"
        );
        let refused = controlling
            .handle
            .spawn_attached_forked_participant(
                MobControlPrincipal::Owner,
                reference,
                attachment("tampered"),
                branch_spec("branch"),
            )
            .await;
        assert!(
            matches!(refused, Err(MobError::ForkedParticipantRefused(_))),
            "a tampered {label} must fail full-reference comparison, got {refused:?}"
        );
    }

    assert!(
        associations(&controlling).await.is_empty(),
        "no refused admission ever took durable custody"
    );
    // The capability is untouched: an ordinary attach still succeeds.
    controlling
        .handle
        .attach_forked_participant(MobControlPrincipal::Owner, &capability, attachment("ok"))
        .await
        .expect("no refused admission consumed the capability's budget");
}

// ===========================================================================
// Teardown
// ===========================================================================

/// Retiring the seated member releases its exact attachment after teardown,
/// exactly once, and discharges the durable association. A repeated release of
/// the same attachment replays instead of consuming the capability again.
#[tokio::test(flavor = "multi_thread")]
async fn retire_releases_the_exact_attachment_once() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-retire").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-retire")
        .await
        .expect("local create");
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("retire-1"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");
    assert_eq!(associations(&controlling).await.len(), 1);

    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("retire the seated branch");
    assert!(
        associations(&controlling).await.is_empty(),
        "a proven release discharges the durable association"
    );
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &capability,
                &attachment("retire-1"),
            )
            .await
            .expect("an exact repeat is admitted"),
        ForkedParticipantReleaseOutcome::Replayed,
        "teardown released exactly once; a retry replays rather than releasing twice"
    );
    // An idempotent retire retry must not re-release either.
    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("retiring an absent member converges");
    assert!(associations(&controlling).await.is_empty());
}

/// Destroying the mob tears the seated member down and releases its exact
/// attachment as part of that teardown.
#[tokio::test(flavor = "multi_thread")]
async fn destroy_releases_the_seated_attachment() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-destroy").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-destroy")
        .await
        .expect("local create");
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("destroy-1"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");

    controlling
        .handle
        .destroy()
        .await
        .expect("destroy the controlling mob");
    assert!(
        associations(&controlling).await.is_empty(),
        "destroy released the seated attachment and discharged its association"
    );
    assert!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &capability,
                &attachment("destroy-1"),
            )
            .await
            .is_err(),
        "a destroyed mob no longer serves capability commands"
    );
}

// ===========================================================================
// Restart
// ===========================================================================

/// A restart RETAINS the association of a still-seated branch — the member,
/// its fork session, and therefore its lease all survive — and the rehydrated
/// association is what later teardown releases. The still-protected fork
/// session also remains closed to a plain resume.
#[tokio::test(flavor = "multi_thread")]
async fn restart_retains_the_association_and_cleanup_still_releases() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-restart").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-restart")
        .await
        .expect("local create");
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("restart-1"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");

    let controlling = controlling.restart().await;
    let recovered = associations(&controlling).await;
    assert_eq!(
        recovered.len(),
        1,
        "the association survives a restart: {recovered:?}"
    );
    assert_eq!(recovered[0].agent_identity, identity("branch"));
    assert_eq!(recovered[0].association.capability, capability);
    assert!(
        recovered[0].obligation.is_none(),
        "a seated member's rehydrated association owes nothing yet"
    );

    // Containment survives the restart too.
    assert!(matches!(
        controlling
            .handle
            .spawn_spec(
                branch_spec("late-bypass")
                    .with_resume_bridge_session_id(capability.fork_session_id().clone())
            )
            .await,
        Err(MobError::ForkedParticipantResumeRequiresAttachment { .. })
    ));

    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("retire the rehydrated branch");
    assert!(
        associations(&controlling).await.is_empty(),
        "cleanup after a restart releases the rehydrated association"
    );
}

/// A crash between a member's teardown and its capability release leaves an
/// ORPHANED association. Boot recovery releases exactly that attachment and
/// discharges the row; a seated member's row is left alone by the same sweep.
#[tokio::test(flavor = "multi_thread")]
async fn restart_releases_an_orphaned_association() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-orphan").await;
    spawn_local_worker(&controlling, "researcher").await;
    let seated_capability = create(&controlling, "researcher", "req-orphan-seated")
        .await
        .expect("local create for the seated branch");
    let orphan_capability = create(&controlling, "researcher", "req-orphan-lost")
        .await
        .expect("local create for the orphaned attachment");

    let seated = controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &seated_capability,
            attachment("orphan-seated"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");

    // Model the crash window: an attachment was admitted for a member whose
    // teardown completed but whose release never ran.
    controlling
        .handle
        .attach_forked_participant(
            MobControlPrincipal::Owner,
            &orphan_capability,
            attachment("orphan-lost"),
        )
        .await
        .expect("take the attachment the crashed seating would have held");
    let orphan_association =
        meerkat_mob::forked_participant::ForkedParticipantAttachmentAssociation::new(
            orphan_capability.clone(),
            attachment("orphan-lost"),
        );
    let orphan = meerkat_mob::store::MobForkedParticipantMemberAssociation {
        association_key: orphan_association.association_key(),
        agent_identity: identity("vanished"),
        association: orphan_association,
        target: None,
        obligation: Some(meerkat_mob::store::MobForkedParticipantObligationCause::SpawnInFlight),
        detail: "crashed inside the seating window".to_string(),
    };
    controlling
        .storage_metadata
        .put_forked_participant_member_association(&controlling.mob_id, &orphan)
        .await
        .expect("inject the orphaned association a crash would have left");
    assert_eq!(associations(&controlling).await.len(), 2);

    let controlling = controlling.restart().await;
    // Recovery is a startup barrier for command admission, not for direct
    // store reads, so observe the durable outcome rather than assuming order.
    wait_until(
        "boot recovery discharges the orphaned association",
        || async { associations(&controlling).await.len() == 1 },
    )
    .await;
    let recovered = associations(&controlling).await;
    assert_eq!(
        recovered[0].agent_identity,
        identity("branch"),
        "the seated member's association is retained, not swept: {recovered:?}"
    );
    assert_eq!(recovered[0].association.capability, seated.capability);

    // The orphan's attachment really was released: an exact repeat replays.
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &orphan_capability,
                &attachment("orphan-lost"),
            )
            .await
            .expect("an exact repeat is admitted"),
        ForkedParticipantReleaseOutcome::Replayed,
        "recovery released the orphan exactly once"
    );
}

/// Respawn is supersession: retiring the predecessor releases its exact
/// attachment before the replacement is provisioned.
#[tokio::test(flavor = "multi_thread")]
async fn respawn_releases_the_predecessor_attachment() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-respawn").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = create(&controlling, "researcher", "req-respawn")
        .await
        .expect("local create");
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("respawn-1"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");
    assert_eq!(associations(&controlling).await.len(), 1);

    controlling
        .handle
        .respawn(identity("branch"), None)
        .await
        .expect("respawn the seated branch");
    assert!(
        associations(&controlling).await.is_empty(),
        "supersession released the predecessor's attachment"
    );
    assert_eq!(
        controlling
            .handle
            .release_forked_participant(
                MobControlPrincipal::Owner,
                &capability,
                &attachment("respawn-1"),
            )
            .await
            .expect("an exact repeat is admitted"),
        ForkedParticipantReleaseOutcome::Replayed,
        "supersession released exactly once"
    );
}

/// A release that cannot be PROVEN must not publish a completed teardown: the
/// retirement fails, the durable obligation is retained, and an exact retry
/// converges once the release can succeed again.
#[tokio::test(flavor = "multi_thread")]
async fn unprovable_release_blocks_completion_and_retry_converges() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob("fp-attach-release-fault").await;
    spawn_local_worker(&controlling, "researcher").await;
    let capability = controlling
        .handle
        .create_forked_participant(
            MobControlPrincipal::Owner,
            identity("researcher"),
            ForkedParticipantRequestId::new("req-release-fault").expect("canonical request id"),
            None,
            ForkedParticipantOperationScope::Observe,
            ForkedParticipantReusePolicy::OneShot,
            TTL,
        )
        .await
        .expect("local create");
    controlling
        .handle
        .spawn_attached_forked_participant(
            MobControlPrincipal::Owner,
            &capability,
            attachment("fault-1"),
            branch_spec("branch"),
        )
        .await
        .expect("seat the branch");
    let healthy = associations(&controlling).await;
    assert_eq!(healthy.len(), 1);

    // Corrupt the row's reference so the owner's FULL-reference comparison
    // refuses the release. The attachment itself is untouched and still held.
    let mut corrupt = healthy[0].clone();
    corrupt.association =
        meerkat_mob::forked_participant::ForkedParticipantAttachmentAssociation::new(
            tampered(
                &capability,
                "scope",
                serde_json::json!("invoke_and_observe"),
            ),
            attachment("fault-1"),
        );
    controlling
        .storage_metadata
        .put_forked_participant_member_association(&controlling.mob_id, &corrupt)
        .await
        .expect("install the corrupt routing evidence");

    let blocked = controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect_err("an unprovable release may not publish a completed teardown");
    let blocked = match &blocked {
        MobError::SharedRetirementFailure(shared) => shared.as_ref(),
        other => other,
    };
    assert!(
        matches!(
            blocked,
            MobError::ForkedParticipantAttachmentReleaseUnproven { .. }
        ),
        "expected a typed unproven-release refusal, got {blocked:?}"
    );
    let retained = associations(&controlling).await;
    assert_eq!(retained.len(), 1, "the obligation is retained");
    assert_eq!(
        retained[0].obligation,
        Some(meerkat_mob::store::MobForkedParticipantObligationCause::TeardownReleaseUnproven),
        "the retained row names why reconciliation is still owed"
    );

    // Restore correct routing evidence; the exact retry now converges.
    let mut repaired = retained[0].clone();
    repaired.association = healthy[0].association.clone();
    repaired.obligation = None;
    repaired.detail = String::new();
    controlling
        .storage_metadata
        .put_forked_participant_member_association(&controlling.mob_id, &repaired)
        .await
        .expect("repair the routing evidence");
    controlling
        .handle
        .retire(identity("branch"))
        .await
        .expect("the exact retry converges");
    assert!(
        associations(&controlling).await.is_empty(),
        "the retry discharged the durable obligation"
    );
}
