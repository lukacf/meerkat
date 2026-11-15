//! Real mixed local + host temporary-council coverage.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{council_support, support};

use council_support::{
    AmbiguousThenPanicCouncilStore, CouncilFixture, ScriptedCouncilClient, ScriptedTurn, TurnGate,
    identity, participant_profile, role_in_request, user_text,
};
use meerkat_mob::definition::SkillSource;
use meerkat_mob::forked_participant::ForkedParticipantOwnerRoute;
use meerkat_mob::machines::forked_participant_lifecycle::{
    ForkedParticipantCleanupState, ForkedParticipantLifecycleState,
};
use meerkat_mob::store::{ForkedParticipantStore, SqliteMobStores};
use meerkat_mob::{MobBackendKind, MobDefinition, MobRuntimeMode, ProfileBinding, ProfileName};
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilParticipantSpec,
    TemporaryCouncilRequest,
};
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, descriptor_to_bind_request, spawn_host_daemon_fixture,
};

const WAIT: Duration = Duration::from_secs(30);

fn portable_definition(id: &str) -> MobDefinition {
    let mut profile = participant_profile("mixed council participant");
    profile.skills = vec!["council-base".to_string()];
    let mut definition = MobDefinition::explicit(id);
    // The shared acceptor is the controller's reverse lane. Do not let the
    // definition's private bridge default shadow this process-wide binding.
    if let Some(external) = definition.backend.external.as_mut() {
        external.supervisor_bridge = None;
    }
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition.skills.insert(
        "council-base".to_string(),
        SkillSource::Inline {
            content: "Answer the assigned council prompt concisely.".to_string(),
        },
    );
    definition
}

fn local_script(
    gate: Arc<TurnGate>,
) -> impl Fn(&meerkat_client::LlmRequest) -> ScriptedTurn + Send + Sync {
    move |request| {
        if user_text(request).contains("Council topic:") {
            ScriptedTurn::Gated(
                gate.clone(),
                format!(
                    "local position from {}",
                    role_in_request(request).unwrap_or_else(|| "unknown".to_string())
                ),
            )
        } else {
            ScriptedTurn::Text("local source ready".to_string())
        }
    }
}

fn host_script(request: &meerkat_client::LlmRequest) -> ScriptedTurn {
    ScriptedTurn::Text(format!(
        "host position from {}",
        role_in_request(request).unwrap_or_else(|| "unknown".to_string())
    ))
}

async fn capability_cleanup_converged(
    store: &dyn ForkedParticipantStore,
    request_id: &meerkat_mob::forked_participant::ForkedParticipantRequestId,
) -> bool {
    let Ok(Some(record)) = store.load_by_request_id(request_id).await else {
        return false;
    };
    matches!(
        record.machine_state.lifecycle_phase,
        ForkedParticipantLifecycleState::Revoked
            | ForkedParticipantLifecycleState::Expired
            | ForkedParticipantLifecycleState::Exhausted
    ) && record.machine_state.active_attachment_id.is_none()
        && record.machine_state.cleanup_state == ForkedParticipantCleanupState::Complete
        && record.cleanup_debt.is_none()
}

async fn until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mixed_local_and_host_temporary_council_completes_and_releases_everything() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let local_gate = TurnGate::new();
    let fixture = CouncilFixture::new_with(local_script(local_gate.clone()), |state, _root| {
        let acceptor = meerkat_mob::ControllingAcceptorConfig::for_session_service(
            "127.0.0.1:0".parse().expect("loopback controller acceptor"),
            None,
            state.session_service(),
        );
        state.with_controlling_acceptor(acceptor)
    });
    let source_mob_id = fixture.source_mob_id();
    fixture
        .state
        .set_local_forked_participant_sweep_interval(Duration::from_millis(100));
    let source_realm =
        meerkat_core::mob_realm_id(source_mob_id.as_str()).expect("source mob's exact realm");

    let host_custody = tempfile::tempdir().expect("host capability custody directory");
    let host_stores = SqliteMobStores::open(host_custody.path().join("capabilities.sqlite3"))
        .expect("open host capability custody");
    let host_capabilities: Arc<dyn ForkedParticipantStore> =
        Arc::new(host_stores.forked_participant_store());
    let host_client = Arc::new(ScriptedCouncilClient::new(host_script));
    let host_calls = host_client.calls();
    let host = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(host_client),
        ..HostFixtureOptions::named("mixed-council-host")
            .with_member_build()
            .with_forked_participant_substrate(Arc::clone(&host_capabilities), source_realm)
            .with_forked_participant_sweep_interval(Duration::from_millis(100))
    })
    .await
    .expect("real member-build host");

    fixture
        .state
        .mob_create_definition_with_owner_bridge_session(
            portable_definition(source_mob_id.as_str()),
            meerkat_core::SessionId::new(),
            false,
            false,
        )
        .await
        .expect("create source mob");
    fixture
        .state
        .mob_spawn(
            &source_mob_id,
            ProfileName::from("participant"),
            identity("local-source"),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("seat local source");
    let source = fixture
        .state
        .handle_for(&source_mob_id)
        .await
        .expect("source handle");
    let source_host = source
        .bind_host(descriptor_to_bind_request(&host.current_descriptor()))
        .await
        .expect("bind host to source mob");
    fixture
        .state
        .mob_spawn(
            &source_mob_id,
            ProfileName::from("participant"),
            identity("host-source"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            Some(source_host.host_id.into()),
        )
        .await
        .expect("seat host-placed source");

    let request = TemporaryCouncilRequest::new(
        fixture.council_id("mixed-real-host"),
        portable_definition("template-is-replaced"),
        vec![
            TemporaryCouncilParticipantSpec::new(
                0,
                "local",
                source_mob_id.clone(),
                identity("local-source"),
                identity("local-branch"),
                ProfileName::from("participant"),
            ),
            TemporaryCouncilParticipantSpec::new(
                1,
                "host",
                source_mob_id.clone(),
                identity("host-source"),
                identity("host-branch"),
                ProfileName::from("participant"),
            ),
        ],
        "Prove the mixed council route.",
        TemporaryCouncilBounds::relative(Duration::from_secs(120), 1, 1024),
        MergeBackPolicy::NoMerge,
    );
    let temporary_mob_id = request.council_id.temporary_mob_id();

    let coordinator = fixture.state.temporary_council();
    let running = tokio::spawn(async move { coordinator.run(request).await });

    local_gate.wait_entered(1).await;
    let temporary = fixture
        .state
        .handle_for(&temporary_mob_id)
        .await
        .expect("temporary mob remains live during the bounded local exchange");
    let roster = temporary.list_members().await;
    assert_eq!(roster.len(), 2, "both source branches are seated");
    for member in &roster {
        assert_eq!(
            member.wired_to.len(),
            1,
            "{} must have the other branch in the completed full mesh",
            member.agent_identity
        );
    }
    let host_live = host.host_binding_record(temporary_mob_id.as_str()).await;
    assert!(
        host_live
            .materialized
            .get("host-branch")
            .and_then(|entry| entry.forked_participant_attachment.as_ref())
            .is_some(),
        "host durable record carries the temporary capability association"
    );

    // Release the test's handle before the council's own cleanup destroys the
    // temporary mob (the 0.9 observer drain refuses a destroy while a MobHandle
    // observes the database).
    drop(temporary);
    local_gate.open();
    let outcome = running
        .await
        .expect("council task joins")
        .expect("mixed council completes");
    assert_eq!(
        outcome.result.exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::Completed
    );
    assert_eq!(outcome.result.rounds_completed, 1);
    assert_eq!(outcome.result.exchanges.len(), 2);
    assert!(
        outcome.result.exchanges.iter().any(|exchange| exchange
            .completed_text()
            .is_some_and(|text| text.contains("local position"))),
        "the local exchange completed"
    );
    assert!(
        outcome.result.exchanges.iter().any(|exchange| exchange
            .completed_text()
            .is_some_and(|text| text.contains("host position"))),
        "the host exchange completed"
    );
    assert_eq!(host_calls.load(Ordering::SeqCst), 1);

    let local = outcome.result.participants[0]
        .capability
        .as_ref()
        .expect("local capability provenance");
    let remote = outcome.result.participants[1]
        .capability
        .as_ref()
        .expect("host capability provenance");
    assert!(matches!(
        &local.owner_route,
        ForkedParticipantOwnerRoute::Local { .. }
    ));
    assert!(matches!(
        &remote.owner_route,
        ForkedParticipantOwnerRoute::Host { .. }
    ));
    assert!(
        outcome.cleanup.settled(),
        "all participant cleanup must converge: {:?}",
        outcome.cleanup
    );
    assert!(
        fixture.state.handle_for(&temporary_mob_id).await.is_err(),
        "the temporary mob was removed"
    );
    assert!(
        host.host_binding_records()
            .await
            .iter()
            .all(|(mob_id, _)| mob_id != temporary_mob_id.as_str()),
        "host teardown removed the temporary mob's materialization and association record"
    );

    let local_request = outcome.result.participants[0].capability_request_id.clone();
    let remote_request = outcome.result.participants[1].capability_request_id.clone();
    let local_store = fixture.state.forked_participant_store_for_tests();
    until("local capability cleanup", || {
        capability_cleanup_converged(local_store.as_ref(), &local_request)
    })
    .await;
    until("host capability cleanup", || {
        capability_cleanup_converged(host_capabilities.as_ref(), &remote_request)
    })
    .await;

    source.shutdown().await.expect("tear down source mob actor");
    fixture.teardown().await;
    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_ambiguous_custody_recovers_and_revokes_after_its_source_disappears() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let crash_gate = TurnGate::new();
    let gate_for_store = crash_gate.clone();
    let fixture = CouncilFixture::new_with(
        |_| ScriptedTurn::Text("unused local response".to_string()),
        move |state, root| {
            let path = meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(root);
            let inner: Arc<dyn meerkat_mob::store::TemporaryCouncilStore> = Arc::new(
                meerkat_mob::store::SqliteTemporaryCouncilStore::open(&path)
                    .expect("open durable council custody"),
            );
            let acceptor = meerkat_mob::ControllingAcceptorConfig::for_session_service(
                "127.0.0.1:0".parse().expect("loopback controller acceptor"),
                None,
                state.session_service(),
            );
            // Commit order: 0 claim, 1 the temporary mob's create intent, 2
            // the participant's acquire-intent, 3 its capability. Index 3 is
            // the post-create commit whose completion is lost.
            state
                .with_temporary_council_store(Arc::new(AmbiguousThenPanicCouncilStore::new(
                    inner,
                    3,
                    gate_for_store,
                )))
                .with_controlling_acceptor(acceptor)
        },
    );
    let source_mob_id = fixture.source_mob_id();
    let source_realm =
        meerkat_core::mob_realm_id(source_mob_id.as_str()).expect("source mob's exact realm");
    let host_custody = tempfile::tempdir().expect("host capability custody directory");
    let host_stores = SqliteMobStores::open(host_custody.path().join("capabilities.sqlite3"))
        .expect("open host capability custody");
    let host_capabilities: Arc<dyn ForkedParticipantStore> =
        Arc::new(host_stores.forked_participant_store());
    let host = spawn_host_daemon_fixture(
        HostFixtureOptions::named("mixed-council-recovery-host")
            .with_member_build()
            .with_forked_participant_substrate(Arc::clone(&host_capabilities), source_realm)
            .with_forked_participant_sweep_interval(Duration::from_millis(100)),
    )
    .await
    .expect("real member-build host");

    fixture
        .state
        .mob_create_definition_with_owner_bridge_session(
            portable_definition(source_mob_id.as_str()),
            meerkat_core::SessionId::new(),
            false,
            false,
        )
        .await
        .expect("create source mob");
    let source = fixture
        .state
        .handle_for(&source_mob_id)
        .await
        .expect("source handle");
    let source_host = source
        .bind_host(descriptor_to_bind_request(&host.current_descriptor()))
        .await
        .expect("bind host to source mob");
    fixture
        .state
        .mob_spawn(
            &source_mob_id,
            ProfileName::from("participant"),
            identity("host-source"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            Some(source_host.host_id.into()),
        )
        .await
        .expect("seat host source");

    let request = TemporaryCouncilRequest::new(
        fixture.council_id("remote-ambiguous"),
        portable_definition("template-is-replaced"),
        vec![TemporaryCouncilParticipantSpec::new(
            0,
            "host",
            source_mob_id.clone(),
            identity("host-source"),
            identity("host-branch"),
            ProfileName::from("participant"),
        )],
        "This exchange must never begin.",
        TemporaryCouncilBounds::relative(Duration::from_secs(120), 1, 1024),
        MergeBackPolicy::NoMerge,
    );
    let council_id = request.council_id.clone();
    let remote_request_id = council_id.capability_request_id(0).expect("request id");
    let coordinator = fixture.state.temporary_council();
    let running = tokio::spawn(async move { coordinator.run(request).await });

    crash_gate.wait_entered(1).await;
    let before_crash = fixture
        .state
        .temporary_council()
        .load(&council_id)
        .await
        .expect("load durable custody")
        .expect("council custody exists");
    let custody = before_crash.participant(0).expect("remote custody");
    assert_eq!(
        custody.acquisition,
        meerkat_mob::temporary_council::TemporaryCouncilAcquisition::Ambiguous
    );
    assert!(
        custody.capability_ref.is_some(),
        "the durable ambiguous custody retains the exact remote reference"
    );

    // The host owns the capability record, not the source controller. A
    // recovery that still revokes it proves it uses the persisted reference.
    source.shutdown().await.expect("remove source mob actor");
    crash_gate.open();
    let recovered = running
        .await
        .expect("supervised coordinator task joins")
        .expect("panic becomes a recoverable terminal outcome");
    assert_eq!(
        recovered.result.exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::CoordinatorInterrupted
    );
    assert!(
        recovered.cleanup.settled(),
        "supervised recovery settles cleanup: {:?}",
        recovered.cleanup
    );
    assert_eq!(recovered.cleanup.revoked_participants, vec![0]);
    assert!(
        fixture
            .state
            .handle_for(&council_id.temporary_mob_id())
            .await
            .is_err(),
        "recovery removes the temporary mob"
    );
    until("remote ambiguous capability cleanup", || {
        capability_cleanup_converged(host_capabilities.as_ref(), &remote_request_id)
    })
    .await;

    fixture.teardown().await;
    host.shutdown().await;
}

const STANDING_BODY: &str = "Always state your uncertainty explicitly.";

/// [`portable_definition`] on a model that can represent an ordered
/// mid-conversation System activation (instruction activation needs it).
fn activation_capable_definition(id: &str) -> MobDefinition {
    let mut definition = portable_definition(id);
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = binding.as_inline_mut() {
            profile.model = "claude-opus-5".to_string();
        }
    }
    definition
}

/// A source mob with a local source and a host-placed source; one host
/// instruction is activated on the placed source, on the member host that
/// holds its session.
struct PlacedActivatedSource {
    fixture: CouncilFixture,
    host: support::HostDaemonFixture,
    host_calls: Arc<std::sync::atomic::AtomicUsize>,
    source: meerkat_mob::MobHandle,
    _host_custody: tempfile::TempDir,
}

async fn placed_activated_source(
    policy: Option<meerkat_core::ForkInstructionPolicy>,
) -> PlacedActivatedSource {
    let local_gate = TurnGate::new();
    local_gate.open();
    let fixture = CouncilFixture::new_with(local_script(local_gate), move |state, _root| {
        let acceptor = meerkat_mob::ControllingAcceptorConfig::for_session_service(
            "127.0.0.1:0".parse().expect("loopback controller acceptor"),
            None,
            state.session_service(),
        );
        let state = state.with_controlling_acceptor(acceptor);
        match policy {
            Some(policy) => state.with_fork_instruction_policy(policy),
            None => state,
        }
    });
    let source_mob_id = fixture.source_mob_id();
    let source_realm =
        meerkat_core::mob_realm_id(source_mob_id.as_str()).expect("source mob's exact realm");
    let host_custody = tempfile::tempdir().expect("host capability custody directory");
    let host_stores = SqliteMobStores::open(host_custody.path().join("capabilities.sqlite3"))
        .expect("open host capability custody");
    let host_capabilities: Arc<dyn ForkedParticipantStore> =
        Arc::new(host_stores.forked_participant_store());
    let host_client = Arc::new(ScriptedCouncilClient::new(host_script));
    let host_calls = host_client.calls();
    let host = spawn_host_daemon_fixture(HostFixtureOptions {
        member_llm_client: Some(host_client),
        ..HostFixtureOptions::named("activated-council-host")
            .with_member_build()
            .with_forked_participant_substrate(host_capabilities, source_realm)
    })
    .await
    .expect("real member-build host");

    fixture
        .state
        .mob_create_definition_with_owner_bridge_session(
            activation_capable_definition(source_mob_id.as_str()),
            meerkat_core::SessionId::new(),
            false,
            false,
        )
        .await
        .expect("create source mob");
    fixture
        .state
        .mob_spawn(
            &source_mob_id,
            ProfileName::from("participant"),
            identity("local-source"),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("seat local source");
    let source = fixture
        .state
        .handle_for(&source_mob_id)
        .await
        .expect("source handle");
    let source_host = source
        .bind_host(descriptor_to_bind_request(&host.current_descriptor()))
        .await
        .expect("bind host to source mob");
    fixture
        .state
        .mob_spawn(
            &source_mob_id,
            ProfileName::from("participant"),
            identity("host-source"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            Some(source_host.host_id.into()),
        )
        .await
        .expect("seat host-placed source");

    // The placed source's session lives on the member host: activate there,
    // under the session's runtime turn boundary, as the member door does.
    let record = host.host_binding_record(source_mob_id.as_str()).await;
    let session_id = meerkat_core::SessionId::parse(
        &record
            .materialized
            .get("host-source")
            .expect("the placed source is materialized on the host")
            .session_id,
    )
    .expect("host member session id");
    let member_service = host
        .member_service
        .as_ref()
        .expect("member-build host session service");
    let guard = member_service
        .acquire_runtime_turn_finalization_guard(&session_id)
        .await
        .expect("turn boundary");
    member_service
        .activate_instruction_under_runtime_turn_boundary(
            &session_id,
            meerkat_core::InstructionActivationRequest {
                revision: meerkat_core::InstructionRevisionRef {
                    namespace: meerkat_core::InstructionNamespace::new("host.standing").unwrap(),
                    key: meerkat_core::InstructionKey::new("member").unwrap(),
                    revision_id: meerkat_core::InstructionRevisionId::new("rev-standing-1")
                        .unwrap(),
                    content_sha256: meerkat_core::InstructionContentDigest::for_body(STANDING_BODY),
                },
                activation_id: meerkat_core::InstructionActivationId::new("standing-1").unwrap(),
                expectation: meerkat_core::InstructionActivationExpectation::Absent,
                supersedes: None,
                body: STANDING_BODY.to_string(),
            },
            None,
        )
        .await
        .expect("the host activates an instruction on the placed source");
    drop(guard);
    PlacedActivatedSource {
        fixture,
        host,
        host_calls,
        source,
        _host_custody: host_custody,
    }
}

fn placed_activated_request(fixture: &CouncilFixture, id: &str) -> TemporaryCouncilRequest {
    let source_mob_id = fixture.source_mob_id();
    TemporaryCouncilRequest::new(
        fixture.council_id(id),
        activation_capable_definition("template-is-replaced"),
        vec![
            TemporaryCouncilParticipantSpec::new(
                0,
                "local",
                source_mob_id.clone(),
                identity("local-source"),
                identity("local-branch"),
                ProfileName::from("participant"),
            ),
            TemporaryCouncilParticipantSpec::new(
                1,
                "host",
                source_mob_id,
                identity("host-source"),
                identity("host-branch"),
                ProfileName::from("participant"),
            ),
        ],
        "Prove the placed fork instruction policy.",
        TemporaryCouncilBounds::relative(Duration::from_secs(120), 1, 1024),
        MergeBackPolicy::NoMerge,
    )
}

/// A participant forked from a host-placed member with an activated host
/// instruction: under the default policy the member host refuses the fork
/// before committing a child, and the council ends with the same typed
/// refusal a local source gives.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_source_participant_is_refused_typed_by_default() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let placed = placed_activated_source(None).await;
    let outcome = placed
        .fixture
        .state
        .temporary_council()
        .run(placed_activated_request(&placed.fixture, "placed-default"))
        .await
        .expect("the council reaches a terminal outcome");
    match &outcome.result.exit_reason {
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::ParticipantForkInstructionRefused {
            participant_order,
            refusal,
        } => {
            assert_eq!(*participant_order, 1, "the placed source's participant");
            assert_eq!(
                refusal.cause,
                meerkat_core::ForkInstructionRefusalCause::PolicyRefuses
            );
        }
        other => panic!("expected the typed participant fork instruction refusal, got {other:?}"),
    }
    assert_eq!(
        placed.host_calls.load(Ordering::SeqCst),
        0,
        "no participant turn ran on the member host"
    );
    placed
        .source
        .shutdown()
        .await
        .expect("tear down source mob actor");
    placed.fixture.teardown().await;
    placed.host.shutdown().await;
}

/// With `ReactivateInherited` the member host re-activates the inherited key
/// in the participant it forks, and the placed participant's council turn
/// reaches the host's model.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_source_participant_runs_when_the_host_opts_in() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let placed = placed_activated_source(Some(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    ))
    .await;
    let outcome = placed
        .fixture
        .state
        .temporary_council()
        .run(placed_activated_request(
            &placed.fixture,
            "placed-reactivate",
        ))
        .await
        .expect("the council reaches a terminal outcome");
    assert_eq!(
        outcome.result.exit_reason,
        meerkat_mob::temporary_council::TemporaryCouncilExitReason::Completed,
        "the council completes with the re-activated placed participant"
    );
    assert!(
        outcome.result.exchanges.iter().any(|exchange| exchange
            .completed_text()
            .is_some_and(|text| text.contains("host position"))),
        "the placed participant took its council turn"
    );
    assert_eq!(placed.host_calls.load(Ordering::SeqCst), 1);
    placed
        .source
        .shutdown()
        .await
        .expect("tear down source mob actor");
    placed.fixture.teardown().await;
    placed.host.shutdown().await;
}
