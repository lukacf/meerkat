//! A mob's start whose startup binding pass meets a member session binding
//! left behind. The route the start takes in production: a member's boot
//! restore succeeded and bound its operation owner, then, before the actor's
//! startup binding pass, the runtime unregistered the member's registration
//! (a run failed and the runtime loop's teardown completed its unregister).
//! The pass registers the session again and replaces the binding, whose
//! registry the unregister retired, instead of refusing the start. A second
//! row covers, through a test seam, the release of the binding a failed
//! member provision installed.

use super::*;
use crate::runtime::MemberRestorePolicy;
use crate::runtime::ops_adapter;
use crate::runtime::provisioner::provision_finish_test_hooks::{self, ProvisionFinishFault};

const STEP: Duration = Duration::from_secs(60);

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// The production route. A Running mob restarts and its builder restores the
/// member, which binds the member's operation owner to that registration.
/// Before the actor's startup binding pass, a run on the member's session
/// never starts executing: the execution-start bound ends it, the runtime
/// loop exits requiring its unregister, and the teardown's unregister
/// removes the registration and retires its registry. The pass then
/// registers the session again. Before the fix it met the restore's binding,
/// which named the retired registry, refused it as "a different
/// operation-registry binding incarnation", and the whole start fell to
/// Stopped. Now it replaces that stale binding with one to the new
/// registration's registry, and the start comes up Running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_rebinds_a_member_whose_registration_a_failed_run_unregistered() {
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let member = AgentIdentity::from("unregistered-during-its-start");
    let peer = AgentIdentity::from("served-through-the-start");
    let paused = paused_running_start(
        &service,
        &adapter,
        &member,
        crate::MobRuntimeMode::TurnDriven,
        std::slice::from_ref(&peer),
    )
    .await;
    let session = paused.session_id.clone();
    let restored_registry = adapter
        .ops_lifecycle_registry(&session)
        .await
        .expect("the restore registered the member's session");
    let (readiness_reached, readiness_release) =
        crate::runtime::actor::arm_startup_readiness_test_gate(paused.mob_id.clone());

    // A run the member's session never starts executing.
    service.set_start_turn_delay_ms(600_000);
    adapter.set_run_execution_start_bound_for_test(&session, Some(Duration::from_millis(300)));
    let (_admitted, _completion) = adapter
        .accept_input_with_completion(
            &session,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "a run that never starts executing",
                None,
            )),
        )
        .await
        .expect("the member's registration admits the run");
    tokio::time::timeout(STEP, async {
        while !matches!(
            adapter.session_attachment_state(&session).await,
            meerkat_runtime::RuntimeSessionAttachmentState::Unregistered
        ) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the failed run's teardown unregisters the member's registration");
    adapter.set_run_execution_start_bound_for_test(&session, None);
    service.set_start_turn_delay_ms(0);

    paused.release.send(()).expect("release the start");
    let reached = tokio::time::timeout(STEP, readiness_reached).await;
    assert!(
        matches!(reached, Ok(Ok(()))),
        "the start never passed its binding pass; the mob is {:?}",
        paused.resumed.status().await
    );
    // The start holds before its readiness pass: the member's new
    // registration is current, and the binding names its registry.
    let rebound_registry = adapter
        .ops_lifecycle_registry(&session)
        .await
        .expect("the start registered the member's session again");
    assert_ne!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&paused.registration),
        "the start registered the member's session anew"
    );
    assert_eq!(
        ops_adapter::unregistered_session_binding_replacement_for_test(&session),
        Some((
            ops_adapter::ops_registry_address_for_test(&restored_registry),
            ops_adapter::ops_registry_address_for_test(&rebound_registry),
        )),
        "the start replaced the binding to the unregistered registration's registry with one to the new registration's registry"
    );
    readiness_release
        .send(())
        .expect("release the readiness pass");

    assert_eq!(
        tokio::time::timeout(STEP, paused.resumed.status())
            .await
            .expect("the start settles")
            .expect("the mob's status"),
        MobState::Running,
        "a member whose registration was unregistered does not stop the start"
    );
    let served = paused
        .resumed
        .member_status(&peer)
        .await
        .expect("the peer's status");
    assert_eq!(
        served.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "{served:?}"
    );
    let _ = tokio::time::timeout(STEP, paused.resumed.stop()).await;
}

/// Seam coverage of the failed-provision release. Two workers, a crash, and
/// a start that restores every member; a test seam fails one restore right
/// after its generated bind, before its operation claim (no production step
/// fails there). The failed provision releases the binding it installed, so
/// the start's binding pass binds the member's session afresh, replacing
/// nothing, and the mob comes up Running with that member Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_failing_after_its_generated_bind_leaves_the_start_running() {
    let definition = with_unique_mob_id(sample_definition(), "startup-binding-release");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let failing = AgentIdentity::from("restore-fails");
    let healthy = AgentIdentity::from("restore-succeeds");
    for identity in [&failing, &healthy] {
        handle
            .spawn_spec(turn_driven(identity))
            .await
            .expect("spawn a worker");
    }
    let session = handle
        .resolve_bridge_session_id(&failing)
        .await
        .expect("a session-backed member");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let runtime = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);

    provision_finish_test_hooks::fail(&session, ProvisionFinishFault::AfterGeneratedBind);
    let resumed = tokio::time::timeout(
        STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                events,
                runtime_metadata,
            ))
            .with_session_service(restarted.clone())
            .with_member_restore_policy(MemberRestorePolicy::Eager)
            .notify_orchestrator_on_resume(false)
            .resume(),
        ),
    )
    .await
    .expect("the start answers")
    .expect("the start succeeds");
    assert!(
        !provision_finish_test_hooks::armed(&session, ProvisionFinishFault::AfterGeneratedBind),
        "the member's restore reached its generated bind and failed there"
    );

    assert_eq!(
        resumed.status().await.expect("the mob's status"),
        MobState::Running,
        "one member's failed restore does not stop the start"
    );
    let failed = resumed
        .member_status(&failing)
        .await
        .expect("the failed member's status");
    assert_eq!(
        failed.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "{failed:?}"
    );
    let restored = resumed
        .member_status(&healthy)
        .await
        .expect("the healthy member's status");
    assert_eq!(
        restored.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "{restored:?}"
    );
    assert!(
        runtime.contains_session(&session).await,
        "the start's binding pass registered the failed member's session again"
    );
    assert_eq!(
        ops_adapter::unregistered_session_binding_replacement_for_test(&session),
        None,
        "the failed provision released its binding, so the start replaced none"
    );

    // A later operation repairs the member.
    tokio::time::timeout(STEP, resumed.respawn(failing.clone(), None))
        .await
        .expect("the respawn answers")
        .expect("the broken member is respawned");
    let repaired = resumed
        .member_status(&failing)
        .await
        .expect("the repaired member's status");
    assert_eq!(
        repaired.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "{repaired:?}"
    );
    let _ = tokio::time::timeout(STEP, resumed.stop()).await;
}
