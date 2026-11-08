//! Actual runtime completions, with no manufactured Ready or recovery control.
use super::*;
use meerkat_mobkit::rpc_gateway::{GatewayHostCloseReason, GatewayHostError};

#[test]
fn in_process_bootstrap_completion_is_one_use_and_recovery_is_joined() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(async move {
        let mut case = Case::new(parent.enter());
        let body = std::panic::AssertUnwindSafe(async {
            let planning = construction::construct(&mut case, pipes).await;
            let host = case.host.as_ref().unwrap();
            let native = case.native.as_ref().unwrap();
            let before = host.observe_acquisition_recovery().unwrap();
            assert!(!before.selected_owner_control);
            assert_eq!(before.task, toolkit_mobkit::installed_host::acquisition::AcquisitionRecoveryTaskObservation::Absent);
            host.publish_runtime_bootstrap_completed(native).unwrap();
            let after = host.observe_acquisition_recovery().unwrap();
            assert!(after.selected_owner_control && !after.stopped);
            assert!(matches!(after.task,
                toolkit_mobkit::installed_host::acquisition::AcquisitionRecoveryTaskObservation::Running
                | toolkit_mobkit::installed_host::acquisition::AcquisitionRecoveryTaskObservation::Returned));
            assert!(host.publish_runtime_bootstrap_completed(native).is_err());
            assert!(matches!(native.take_bootstrap_completion(),
                Err(meerkat_mobkit::unified_runtime::BootstrapCompletionUnavailable::AlreadyTaken)));
            assert!(planning.script.calls.lock().unwrap().is_empty());
        }).catch_unwind().await;
        let cleanup = case.cleanup_completed_runtime().await;
        if let Err(payload) = body { std::panic::resume_unwind(payload); }
        assert!(cleanup.is_empty(), "actual completion owners must join: {cleanup:?}");
    });
}

#[test]
fn in_process_bootstrap_completion_refuses_foreign_runtime_without_taking_it() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(async move {
        let mut case = Case::new(parent.enter());
        let mut foreign: Option<UnifiedRuntime> = None;
        let body = std::panic::AssertUnwindSafe(async {
            let _planning = construction::construct(&mut case, pipes).await;
            // A separate real runtime and service, with a different native mob
            // name. No duplicate-name takeover or fabricated completion.
            let mut definition = construction::mob_definition();
            definition.id = MobId::from(format!("sdk-foreign-completion-{}", SessionId::new()));
            for binding in definition.profiles.values_mut() {
                if let ProfileBinding::Inline(profile) = binding { profile.tools.rust_bundles.clear(); }
            }
            let namespace = definition.id.to_string();
            let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
            builder.default_llm_client = Some(Arc::new(PlanningScript::default()));
            let service = Arc::new(meerkat_session::EphemeralSessionService::new(
                builder, 4));
            let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service)
                .with_declared_ephemeral_mob_storage();
            foreign = Some(UnifiedRuntime::bootstrap(spec, MetaConfig {
                modules: vec![], discovery: DiscoverySpec { namespace, modules: vec![] }, pre_spawn: vec![],
            }, BOUND).await.unwrap());
            let other = foreign.as_mut().unwrap();
            other.activate_without_identity_context().await.unwrap();
            let host = case.host.as_ref().unwrap();
            assert!(matches!(host.publish_runtime_bootstrap_completed(other),
                Err(GatewayHostError::Denied | GatewayHostError::Unavailable)));
            assert!(other.take_bootstrap_completion().is_ok(), "wrong host must not spend the real completion");
            assert!(!host.observe_acquisition_recovery().unwrap().selected_owner_control);
            host.publish_runtime_bootstrap_completed(case.native.as_ref().unwrap()).unwrap();
            assert!(host.observe_acquisition_recovery().unwrap().selected_owner_control);
        }).catch_unwind().await;
        let mut foreign_clean = true;
        if let Some(foreign) = foreign {
            let report = foreign.shutdown().await;
            foreign_clean = report.cleanup_completed() && matches!(report.mob_terminal_shutdown,
                meerkat_mobkit::MobTerminalShutdownOutcome::Completed(ref report) if report.is_clean());
        }
        let cleanup = case.cleanup_completed_runtime().await;
        if let Err(payload) = body { std::panic::resume_unwind(payload); }
        assert!(foreign_clean, "actual foreign runtime must join");
        assert!(cleanup.is_empty(), "actual completion owners must join: {cleanup:?}");
    });
}

#[test]
fn in_process_bootstrap_completion_refuses_closed_host_without_starting_recovery() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap().block_on(async move {
        let mut case = Case::new(parent.enter());
        let body = std::panic::AssertUnwindSafe(async {
            let _planning = construction::construct(&mut case, pipes).await;
            let host = case.host.as_ref().unwrap();
            host.begin_close(GatewayHostCloseReason::Shutdown).unwrap();
            let native = case.native.as_ref().unwrap();
            assert!(host.publish_runtime_bootstrap_completed(native).is_err());
            assert!(native.take_bootstrap_completion().is_ok(), "closed host refuses before taking completion");
            let observed = host.observe_acquisition_recovery().unwrap();
            assert!(observed.stopped && !observed.selected_owner_control);
            assert_eq!(observed.task, toolkit_mobkit::installed_host::acquisition::AcquisitionRecoveryTaskObservation::Absent);
        }).catch_unwind().await;
        let cleanup = case.cleanup_completed_runtime().await;
        if let Err(payload) = body { std::panic::resume_unwind(payload); }
        assert!(cleanup.is_empty(), "closed actual host must join: {cleanup:?}");
    });
}
