//! Multi-host mobs phase 3 — CONTROLLING-side `RequestMemberMaterialization`
//! realization against a SCRIPTED host (design-spec-compiler-dispatch §1 rows
//! I1–I12, `MaterializeServeMode::Scripted`).
//!
//! TDD-first: written against the phase-3 shapes named by the designs +
//! ADJUDICATIONS (ADJ-1 budget single carrier, ADJ-4 exactly-one-resend,
//! ADJ-6 workgraph observation, ADJ-7 placement threading, ADJ-15 operator
//! authority record); compiles once wave lanes A1/A2 land.
//!
//! Consolidations recorded here (same behavior = one test):
//!   * F T-L12 (local arms untouched)            → `local_spawn_*` below (== I2)
//!   * F T-L5 scripted twin (digest/engine echo)  → I4
//!   * F T-L3 scripted twin (retry dedup)         → I6
//!   * F T-L11 placement-admission reject matrix  → I3 (extended)
//!   * F T-F1 wire pin (budget_seed DELETED)      → I12
//!
//! Spec-compiler output facts (S U1/U3/U4/U6) are pinned through the PUBLIC
//! carrier — the captured `BridgeMaterializePayload` — never through compiler
//! internals; the in-crate `spec_compiler` mod-test twins are the A2 lane's.
//!
//! The OwnerBridgeSessionAbsent denial leg (I3) and the trust-install
//! ORDERING leg (I8) have no honest fixture-level construction; both are
//! machine-kernel-pinned (phase-1 `multi_host_machine.rs`) and in-crate
//! (A2 lane) respectively.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::AgentIdentity;
use meerkat_mob::MobError;
use meerkat_mob::machines::mob_machine::MobSpawnMemberAdmissionKind;
use meerkat_mob::runtime::bridge_protocol::{
    BridgeRejectionCause, MaterializeLaunchMode, PortableSystemPrompt, portable_member_spec_digest,
};
use support::{
    REAL_COMMS_TEST_LOCK, create_controlling_mob, placed_spawn_spec, spawn_scripted_host_peer,
};

const SETTLE: Duration = Duration::from_secs(10);

/// Zero-tool dispatcher: the per-spawn external-tool PRESENCE fact is what
/// the denial arm observes, not the tool set.
struct EmptyDispatcher;

#[async_trait::async_trait]
impl meerkat_core::agent::AgentToolDispatcher for EmptyDispatcher {
    fn tools(&self) -> std::sync::Arc<[std::sync::Arc<meerkat_core::ToolDef>]> {
        Vec::new().into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::ToolError> {
        Err(meerkat_core::error::ToolError::not_found(call.name))
    }
}

// ===========================================================================
// I1 — happy path: placement walks MaterializePending → scripted ack →
// CommitSpawnMembershipRemote publishes the roster FROM THE ACK
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn placed_spawn_materializes_via_bridge_and_commits_from_ack() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i1-scripted-host").await;
    let controlling = create_controlling_mob("i1-remote-spawn").await;
    let report = controlling.bind_scripted(&scripted).await;

    let spawned = controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn must commit from the scripted ack");
    assert_eq!(spawned.agent_identity, AgentIdentity::from("b2"));

    // Exactly ONE MaterializeMember; its digest is the machine-recorded
    // resolved digest (recompute over the carried spec must agree), and the
    // spec decodes with never-Inherit prompt/policy (S U3/U4 public-carrier
    // pins).
    let payloads = scripted.received_materialize_payloads();
    assert_eq!(payloads.len(), 1, "exactly one materialize send");
    let payload = &payloads[0];
    assert_eq!(payload.spec.agent_identity, "b2");
    assert_eq!(
        payload.spec_digest,
        portable_member_spec_digest(&payload.spec).expect("received spec digest recomputes"),
        "the carried digest must be the canonical digest of the carried spec"
    );
    assert!(
        matches!(
            payload.spec.overlay.system_prompt,
            PortableSystemPrompt::Set { .. }
        ) || matches!(
            payload.spec.overlay.system_prompt,
            PortableSystemPrompt::Disable
        ),
        "R3: the compiled overlay prompt is Set/Disable — Inherit is unrepresentable"
    );
    assert_eq!(payload.spec.mob_id, controlling.mob_id.to_string());
    assert_eq!(payload.spec.profile_name, "worker");
    assert!(
        matches!(payload.launch, MaterializeLaunchMode::Fresh {}),
        "fresh spawn maps to launch Fresh"
    );

    // Roster entry is live and the member exists NOWHERE locally: the remote
    // session binding is machine truth from the ack, never a local session.
    let members = controlling.handle.list_members().await;
    assert!(
        members.iter().any(|entry| entry.agent_identity == "b2"),
        "committed placed member must appear in the roster"
    );

    scripted.shutdown();
}

// ===========================================================================
// I2 — local arms untouched: placement-less spawn feeds REAL observations,
// local commit carries no ack fields (== F T-L12)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn local_spawn_with_honest_observations_still_walks_local_arms() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i2-scripted-host").await;
    let controlling = create_controlling_mob("i2-local-regression").await;
    controlling.bind_scripted(&scripted).await;

    // Placement-less spawn on the SAME mob (a bound host exists, so any
    // accidental remote routing would be observable at the scripted peer).
    let spec = meerkat_mob::SpawnMemberSpec::new("worker", "local-1");
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("local spawn must keep working with honest observations threaded");

    assert_eq!(
        scripted.materialize_count(),
        0,
        "a local spawn must never send MaterializeMember"
    );
    let members = controlling.handle.list_members().await;
    assert!(
        members
            .iter()
            .any(|entry| entry.agent_identity == "local-1"),
        "local member committed through the local arms"
    );

    scripted.shutdown();
}

// ===========================================================================
// I3 — shell denial feed (the input-feed-gap lesson): typed
// SpawnMemberAdmissionDenied, NOTHING materializes (== F T-L11 / T-F5(i))
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn placement_admission_denials_are_typed_and_materialize_nothing() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i3-scripted-host").await;
    let controlling = create_controlling_mob("i3-denials").await;
    let report = controlling.bind_scripted(&scripted).await;

    // (i) secret-bearing shell env → SecretBearingShellEnv.
    let mut spec = placed_spawn_spec("worker", "dirty-env", &report.host_id);
    spec.shell_env = Some(std::collections::HashMap::from([(
        "SECRET_TOKEN".to_string(),
        "hunter2".to_string(),
    )]));
    let denied = controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect_err("secret-bearing shell env must deny a placed spawn");
    assert!(
        matches!(
            denied,
            MobError::SpawnMemberAdmissionDenied {
                admission: MobSpawnMemberAdmissionKind::SecretBearingShellEnv,
            }
        ),
        "expected SecretBearingShellEnv denial, got {denied:?}"
    );

    // (ii) EXPLICIT workgraph assertion → NonPortableWorkgraphTools (ADJ-6:
    // the observation is the explicit resolved profile fact only).
    let denied = controlling
        .spawn_placed("workgrapher", "wg-remote", &report.host_id)
        .await
        .expect_err("explicit workgraph tools must deny a placed spawn");
    assert!(
        matches!(
            denied,
            MobError::SpawnMemberAdmissionDenied {
                admission: MobSpawnMemberAdmissionKind::NonPortableWorkgraphTools,
            }
        ),
        "expected NonPortableWorkgraphTools denial, got {denied:?}"
    );

    // (iii) placement names an UNBOUND host → HostNotBound (typed denial,
    // never a panic — garbage id included).
    let denied = controlling
        .spawn_placed("worker", "nowhere", "peer-not-a-bound-host")
        .await
        .expect_err("unbound placement must deny");
    assert!(
        matches!(
            denied,
            MobError::SpawnMemberAdmissionDenied {
                admission: MobSpawnMemberAdmissionKind::HostNotBound,
            }
        ),
        "expected HostNotBound denial, got {denied:?}"
    );

    // (iv) per-spawn external tools → NonPortablePerSpawnExternalTools.
    let mut spec = placed_spawn_spec("worker", "ext-tools", &report.host_id);
    spec.external_tools = Some(std::sync::Arc::new(EmptyDispatcher));
    let denied = controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect_err("per-spawn external tools must deny a placed spawn");
    assert!(
        matches!(
            denied,
            MobError::SpawnMemberAdmissionDenied {
                admission: MobSpawnMemberAdmissionKind::NonPortablePerSpawnExternalTools,
            }
        ),
        "expected NonPortablePerSpawnExternalTools denial, got {denied:?}"
    );

    // Every denial fired BEFORE any wire dispatch and left no roster residue.
    assert_eq!(
        scripted.materialize_count(),
        0,
        "denied spawns must never materialize"
    );
    let members = controlling.handle.list_members().await;
    assert!(
        members.is_empty(),
        "denied spawns must leave no roster entry, got {members:?}"
    );

    scripted.shutdown();
}

// ===========================================================================
// I3b — placement and explicit backend/binding are mutually exclusive
// (DEC-P3-3: typed WiringError BEFORE any machine input)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn placement_with_explicit_backend_is_a_typed_wiring_error() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i3b-scripted-host").await;
    let controlling = create_controlling_mob("i3b-wiring").await;
    let report = controlling.bind_scripted(&scripted).await;

    let mut spec = placed_spawn_spec("worker", "conflicted", &report.host_id);
    spec.backend = Some(meerkat_mob::MobBackendKind::External);
    let error = controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect_err("placement + explicit backend must be mutually exclusive");
    assert!(
        matches!(error, MobError::WiringError(_)),
        "expected typed WiringError, got {error:?}"
    );
    assert_eq!(scripted.materialize_count(), 0);

    scripted.shutdown();
}

// ===========================================================================
// I4 — digest echo mismatch / engine-version mismatch refuse the commit
// (machine guards spec_digest_echo_matches / engine_version_matches_bound_host)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn doctored_ack_digest_or_engine_version_never_commits() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i4-scripted-host").await;
    let controlling = create_controlling_mob("i4-echo-mismatch").await;
    let report = controlling.bind_scripted(&scripted).await;

    scripted.echo_wrong_digest_next_materialize();
    let error = controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect_err("a doctored spec_digest echo must refuse the commit");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("digest"),
        "the typed error names the digest mismatch, got {rendered}"
    );
    assert!(
        controlling.handle.list_members().await.is_empty(),
        "no roster entry may exist after a refused commit"
    );
    assert!(
        scripted.scripted_member_rows().is_empty(),
        "digest mismatch compensation must release the exact host member before returning"
    );
    assert!(
        scripted
            .received_release_payloads()
            .iter()
            .any(|payload| payload.agent_identity == "b2"),
        "digest mismatch compensation must issue an exact host release"
    );
    assert!(
        controlling
            .storage_metadata
            .load_placed_spawn(&controlling.mob_id, "b2")
            .await
            .expect("load digest-mismatch carrier")
            .is_none(),
        "digest mismatch compensation must delete the exact Pending carrier"
    );

    // Engine-version twin (§15 R8: HostEngineVersionChanged-class failure).
    scripted.echo_engine_version_next_materialize("some-other-engine");
    let error = controlling
        .spawn_placed("worker", "b2-again", &report.host_id)
        .await
        .expect_err("an ack naming a different engine version must refuse the commit");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("engine"),
        "the typed error names the engine-version mismatch, got {rendered}"
    );
    assert!(controlling.handle.list_members().await.is_empty());
    assert!(
        scripted.scripted_member_rows().is_empty(),
        "engine mismatch compensation must release the exact host member before returning"
    );
    assert!(
        scripted
            .received_release_payloads()
            .iter()
            .any(|payload| payload.agent_identity == "b2-again"),
        "engine mismatch compensation must issue an exact host release"
    );
    assert!(
        controlling
            .storage_metadata
            .load_placed_spawn(&controlling.mob_id, "b2-again")
            .await
            .expect("load engine-mismatch carrier")
            .is_none(),
        "engine mismatch compensation must delete the exact Pending carrier"
    );

    scripted.shutdown();
}

// ===========================================================================
// I5 — typed host reject: terminal class, NO resend, failure recorded from
// the wire cause tag (DEC-P3-7), abort clears the rung
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn typed_host_reject_aborts_without_resend_and_carries_the_cause() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i5-scripted-host").await;
    let controlling = create_controlling_mob("i5-typed-reject").await;
    let report = controlling.bind_scripted(&scripted).await;

    scripted.reject_next_materialize(
        BridgeRejectionCause::ModelUnresolvable {
            model: "claude-haiku-4-5-20251001".to_string(),
        },
        "injected: model unknown on scripted host",
    );
    scripted.fail_next_release(
        BridgeRejectionCause::Unsupported,
        "unknown old-binding tuple",
    );
    let status_count_before_cleanup = scripted.host_status_count();
    let error = controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect_err("a typed host reject must fail the spawn");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("ModelUnresolvable") || rendered.contains("model_unresolvable"),
        "the spawner-visible error carries the wire cause, got {rendered}"
    );
    assert_eq!(
        scripted.materialize_count(),
        1,
        "a terminal-class reject must NOT be resent (ADJ-4: resend only on timeout/Unavailable)"
    );
    assert_eq!(
        scripted.release_count(),
        1,
        "UnknownMember cleanup rejection must not be retried before exact absence proof",
    );
    assert!(
        scripted.host_status_count() > status_count_before_cleanup,
        "authenticated Unsupported/UnknownMember must advance to HostStatus exact-absence certification",
    );
    assert!(controlling.handle.list_members().await.is_empty());

    scripted.shutdown();
}

// ===========================================================================
// I6 — reply-lost retry idempotence: exactly ONE resend at the SAME tuple,
// byte-identical payloads, dedup replay commits (ADJ-4)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn dropped_reply_resends_once_at_the_same_idempotency_tuple() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i6-scripted-host").await;
    let controlling = create_controlling_mob("i6-retry").await;
    let report = controlling.bind_scripted(&scripted).await;

    scripted.drop_next_materialize_replies(1);
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the single resend must converge through the host dedup replay");

    let payloads = scripted.received_materialize_payloads();
    assert_eq!(payloads.len(), 2, "original send + exactly one resend");
    assert_eq!(
        payloads[0], payloads[1],
        "the resend rides the SAME (identity, generation, fence) tuple, digest and spec bytes"
    );
    let members = controlling.handle.list_members().await;
    assert_eq!(
        members
            .iter()
            .filter(|entry| entry.agent_identity == "b2")
            .count(),
        1,
        "exactly one roster entry despite two sends"
    );

    scripted.shutdown();
}

// ===========================================================================
// I7 — timeout exhaustion → RecordMemberMaterializationFailure + abort +
// documented orphan seed, then convergence through the shared HostStatus
// sweep (sibling row `host_orphan_reconciliation.rs`)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn exhausted_resend_budget_aborts_typed_and_reconciles_the_orphan_seed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i7-scripted-host").await;
    let controlling = create_controlling_mob("i7-timeout").await;
    let report = controlling.bind_scripted(&scripted).await;

    // Both the original send and the single ADJ-4 resend lose their replies.
    scripted.drop_next_materialize_replies(2);
    let error = controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect_err("exhausted resend budget must fail the spawn typed");
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("timeout") || rendered.contains("Timeout"),
        "the typed failure carries the materialize_timeout class, got {rendered}"
    );

    assert_eq!(
        scripted.materialize_count(),
        2,
        "exactly one resend before exhaustion (ADJ-4)"
    );
    let materialize = scripted.received_materialize_payloads();
    assert_eq!(materialize.len(), 2);
    assert_eq!(
        materialize[0], materialize[1],
        "the one resend preserves the exact idempotency tuple"
    );
    assert!(
        controlling.handle.list_members().await.is_empty(),
        "the aborted spawn must leave no roster entry"
    );
    // The host recorded the build, but the shared periodic sweep may reclaim
    // it before the 2x60s materialize timeout returns to this task. Prove the
    // orphan seed by the exact release receipt rather than a timing-sensitive
    // intermediate inventory row.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if scripted.scripted_member_rows().is_empty()
                && !scripted.received_release_payloads().is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the shared HostStatus sweep reconciles the orphan seed");
    let releases = scripted.received_release_payloads();
    assert_eq!(
        releases.len(),
        1,
        "the orphan seed is released exactly once"
    );
    assert_eq!(
        releases[0].agent_identity,
        materialize[0].spec.agent_identity
    );
    assert_eq!(releases[0].generation, materialize[0].generation);
    assert_eq!(releases[0].fence_token, materialize[0].fence_token);
    assert!(scripted.scripted_member_rows().is_empty());

    scripted.shutdown();
}

// ===========================================================================
// I9 — operator-authority projection: only the canonical COMMITTED carrier
// projects member authority; Pending/aborted attempts never grant.
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn operator_authority_record_written_for_mob_tools_and_cleared_on_abort() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i9-scripted-host").await;
    let controlling = create_controlling_mob("i9-authority-record").await;
    let report = controlling.bind_scripted(&scripted).await;

    // tools.mob = true ("worker" profile) → record present after commit.
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn commits");
    let b2_record = controlling
        .storage_metadata
        .load_committed_placed_member_operator_authority(&controlling.mob_id, "b2", 0)
        .await
        .expect("load committed member operator authority")
        .expect("tools.mob=true placed member must have a durable operator-authority record");
    // The record projects the MINTED context facts verbatim (plan §15.4;
    // design-S §3.2: mint via the build.rs resolve_profile_mob_operator_access
    // path). The frozen generated mint (`ResolveMobOperatorCreateAuthority`)
    // grants create + profile-mutation authority for `tools.mob = true`, and
    // the default spawn-profile grant covers the definition's profile names.
    assert!(
        b2_record.can_create_mobs && b2_record.can_mutate_profiles,
        "the record mirrors the generated create-authority mint"
    );
    assert!(
        b2_record
            .spawn_profile_scope
            .get(controlling.mob_id.as_str())
            .is_some_and(|profiles| profiles.contains("worker")),
        "the default spawn-profile grant covers the definition's profiles"
    );

    // tools.mob = false → NO record.
    controlling
        .spawn_placed("quiet-worker", "b3", &report.host_id)
        .await
        .expect("quiet placed spawn commits");
    let b3_record = controlling
        .storage_metadata
        .load_committed_placed_member_operator_authority(&controlling.mob_id, "b3", 0)
        .await
        .expect("load quiet member operator authority");
    assert!(
        b3_record.is_none(),
        "tools.mob=false must write no operator-authority record"
    );

    // Abort path deletes the record: a typed TERMINAL host reject on a
    // mob-tools spawn. (`Unavailable` is ADJ-4 resend-class — the single
    // resend would legitimately commit against the recovered host.)
    scripted.reject_next_materialize(BridgeRejectionCause::Internal, "injected reject");
    let _ = controlling
        .spawn_placed("worker", "b4", &report.host_id)
        .await
        .expect_err("rejected spawn aborts");
    let b4_record = controlling
        .storage_metadata
        .load_committed_placed_member_operator_authority(&controlling.mob_id, "b4", 0)
        .await
        .expect("load aborted member operator authority");
    assert!(
        b4_record.is_none(),
        "the abort path must leave the operator-authority record deleted"
    );

    scripted.shutdown();
}

// ===========================================================================
// I10 — cancellation: mob destroy with a materialize dispatch in flight
// aborts the pending spawn at the MaterializePending rung
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn destroy_with_materialize_in_flight_aborts_the_pending_spawn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i10-scripted-host").await;
    let controlling = create_controlling_mob("i10-cancel").await;
    let report = controlling.bind_scripted(&scripted).await;

    // Park the dispatch: both replies dropped, so the spawn sits in its
    // send/resend window while destroy runs.
    scripted.drop_next_materialize_replies(2);
    let handle = controlling.handle.clone();
    let host_id = report.host_id.clone();
    let spawn_task = tokio::spawn(async move {
        handle
            .spawn_spec(support::placed_spawn_spec("worker", "b2", &host_id))
            .await
    });
    // Give the dispatch a chance to leave (notify-driven would be better but
    // the scripted peer records receipt synchronously with serving).
    tokio::time::timeout(SETTLE, async {
        loop {
            if scripted.materialize_count() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("materialize dispatch observed");

    controlling
        .handle
        .destroy()
        .await
        .expect("destroy with a spawn in flight must complete");
    let spawn_result = spawn_task.await.expect("spawn task joins");
    assert!(
        spawn_result.is_err(),
        "the in-flight spawn must fail typed on destroy, got {spawn_result:?}"
    );

    scripted.shutdown();
}

// ===========================================================================
// I11 — launch mapping: Resume{id} + placement rides the payload and skips
// the local resume fast-path; Fork + placement collapses to launch Fresh
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn launch_modes_map_to_wire_launch_without_local_probes() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i11-scripted-host").await;
    let controlling = create_controlling_mob("i11-launch").await;
    let report = controlling.bind_scripted(&scripted).await;

    // Resume of a session id that exists NOWHERE on the controlling host:
    // had the local resume fast-path run, this spawn would fail on the local
    // probe — success + the payload shape IS the pin (validation is
    // realm-local to the member host, A6).
    let remote_session = meerkat_core::SessionId::new();
    let mut spec = support::placed_spawn_spec("worker", "resumer", &report.host_id);
    spec.launch_mode = meerkat_mob::launch::MemberLaunchMode::Resume {
        bridge_session_id: remote_session.clone(),
    };
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed resume spawn must not consult the local session store");
    let payloads = scripted.received_materialize_payloads();
    let resume_payload = payloads
        .iter()
        .find(|payload| payload.spec.agent_identity == "resumer")
        .expect("resume materialize payload");
    assert!(
        matches!(
            &resume_payload.launch,
            MaterializeLaunchMode::Resume { session_id } if *session_id == remote_session.to_string()
        ),
        "Resume launch rides the payload verbatim, got {:?}",
        resume_payload.launch
    );

    // Fork from a LOCAL source member collapses to launch Fresh on the wire
    // (§19.L2: fork is prompt text by the time it leaves the controlling
    // host; the rendered-fork-prompt content pin is the ladder file's).
    controlling
        .handle
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", "fork-source"))
        .await
        .expect("local fork source spawns");
    let mut spec = support::placed_spawn_spec("worker", "forked", &report.host_id);
    spec.launch_mode = meerkat_mob::launch::MemberLaunchMode::Fork {
        source_member_id: AgentIdentity::from("fork-source"),
        fork_context: meerkat_mob::launch::ForkContext::default(),
    };
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed fork spawn commits");
    let payloads = scripted.received_materialize_payloads();
    let fork_payload = payloads
        .iter()
        .find(|payload| payload.spec.agent_identity == "forked")
        .expect("fork materialize payload");
    assert!(
        matches!(fork_payload.launch, MaterializeLaunchMode::Fresh {}),
        "Fork is unrepresentable on this wire — it collapses to Fresh, got {:?}",
        fork_payload.launch
    );

    scripted.shutdown();
}

// ===========================================================================
// I12 — budget single carrier (ADJ-1): the compiled overlay carries the spec
// budget; the payload struct has NO budget_seed (deny_unknown_fields reject)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn budget_rides_the_digest_covered_overlay_and_nowhere_else() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("i12-scripted-host").await;
    // The label must not contain the "budget" token: the mob id is embedded
    // verbatim in the payload (spec.mob_id, authority-context scope key,
    // supervisor name) and would inflate the single-carrier substring count
    // below with non-budget matches.
    let controlling = create_controlling_mob("i12-single-carrier").await;
    let report = controlling.bind_scripted(&scripted).await;

    let limits = meerkat_core::BudgetLimits {
        max_tokens: None,
        max_duration: None,
        max_tool_calls: Some(1),
    };
    let mut spec = support::placed_spawn_spec("worker", "b2", &report.host_id);
    spec.budget_limits = Some(limits.clone());
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("budgeted placed spawn commits");

    let payloads = scripted.received_materialize_payloads();
    let payload = payloads
        .iter()
        .find(|payload| payload.spec.agent_identity == "b2")
        .expect("budgeted materialize payload");
    assert_eq!(
        payload.spec.overlay.budget_limits.as_ref(),
        Some(&limits),
        "the spec budget lands in the digest-covered overlay (single carrier)"
    );
    // The serialized payload carries NO second budget value anywhere outside
    // the overlay: the only "budget" token in the JSON is the overlay field.
    let raw = serde_json::to_value(payload).expect("payload serializes");
    let rendered = raw.to_string();
    assert_eq!(
        rendered.matches("budget").count(),
        1,
        "exactly one budget carrier on the wire, got payload {rendered}"
    );

    // ADJ-1 wire pin (the BudgetSplitPolicy absence-pin pattern): a payload
    // carrying the DELETED `budget_seed` field must fail decode.
    let mut smuggled = raw;
    smuggled.as_object_mut().expect("payload object").insert(
        "budget_seed".to_string(),
        serde_json::json!({ "max_tool_calls": 9 }),
    );
    let decode: Result<meerkat_mob::runtime::bridge_protocol::BridgeMaterializePayload, _> =
        serde_json::from_value(smuggled);
    assert!(
        decode.is_err(),
        "BridgeMaterializePayload must deny_unknown_fields-reject the deleted budget_seed"
    );

    scripted.shutdown();
}
