//! The actual installed host hook rejects an unusable native weak back-link
//! before consuming its slot. No identity/member/input is registered here.
use super::*;
use crate::private_originals::PrivateOriginalError;
use futures::FutureExt;
use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_core::service::{SessionQuery, SessionService};
use meerkat_mob::{
    MobDefinition, MobId, MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName,
    ToolConfig,
};
use meerkat_mobkit::{
    DiscoverySpec, MetaConfig, MobBootstrapSpec, MobTerminalShutdownOutcome, UnifiedRuntime,
    identity_first::{
        AgentRuntimeServices, DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
        MutableRosterProvider,
    },
    rpc_gateway::{GatewayHostCloseReason, GatewayHostComposition, GatewayHostError},
};
use meerkat_runtime::MeerkatMachine;
use std::time::Duration;
use toolkit_store::ToolkitStore;

fn definition() -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from("factory"));
    definition.profiles.insert(
        ProfileName::from("identity"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "claude-sonnet-4-5".into(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: ToolConfig { comms: true, ..Default::default() },
            peer_description: "unspawned identity binding control".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    definition
}

fn context(native: &UnifiedRuntime, with_services: bool) -> Arc<IdentityFirstRuntimeContext> {
    let handle = native.mob_handle();
    let identity = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "toolkit-bind-slot-control".into(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(handle.observer()))),
        default_timeout: None,
    });
    let identity = if with_services {
        identity.with_runtime_services(AgentRuntimeServices::new(handle.clone()))
    } else { identity };
    Arc::new(IdentityFirstRuntimeContext::new(
        Arc::new(identity), Arc::new(MutableRosterProvider::new(Vec::new())),
        None, None, Some(handle.definition().clone()),
    ))
}

#[tokio::test]
async fn unavailable_identity_binding_preserves_slot_and_success_still_denies_rebind() {
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let (owner, installed) = InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
        .await.unwrap();
    let directory = state_directory(&selection, &installed.native, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let originals = installed.originals.clone();
    let store = installed.store.clone();
    let scope = installed.source_scope.clone();
    let policy_rows = store.list_records(&scope, None, 32).await.unwrap();
    assert_eq!(policy_rows.len(), 1);
    assert_eq!(policy_rows[0].data.schema, "toolkit.application-service-tool-policy");
    let (_signal, closed) = tokio::sync::watch::channel(false);
    let owner = Arc::new(owner);
    let host = installed.compose(owner.clone(), PrivateHostClosed(closed));
    let configuration = host.native_grant_work_configuration().unwrap().unwrap();
    let machine = Arc::new(MeerkatMachine::ephemeral()
        .with_local_grant_authorization(configuration).unwrap());
    let service = Arc::new(EphemeralSessionService::new(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()), 1,
    ));
    let spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
        .with_session_runtime_adapter(machine).unwrap()
        .with_declared_ephemeral_mob_storage();
    let spec = host.prepare_bootstrap(spec).await.unwrap();
    let mut native = UnifiedRuntime::bootstrap(spec, MetaConfig {
        modules: Vec::new(),
        discovery: DiscoverySpec { namespace: "factory".into(), modules: Vec::new() },
        pre_spawn: Vec::new(),
    }, Duration::from_secs(10)).await.unwrap();

    let checked = std::panic::AssertUnwindSafe(async {
        host.bind_runtime(&mut native).await.unwrap();
        assert!(owner.validate_current_selection().is_ok());
        assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));

        // This is an actual native view with a live strong runtime but absent
        // runtime services. The public weak contract must reject it.
        let absent = context(&native, false);
        native.install_identity_first_context(absent);
        let candidate = native.identity_runtime().unwrap();
        assert!(candidate.downgrade().upgrade().is_none());
        assert!(matches!(host.bind_identity_runtime(&mut native).await,
            Err(GatewayHostError::Unavailable)));
        assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
        assert_eq!(store.list_records(&scope, None, 32).await.unwrap(), policy_rows,
            "identity binding preserves the exact selected service policy row");
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grant_bytes);
        drop(candidate);

        // Correct the native composition through its existing public context
        // entry, with that same native mob's actual handle and no new member.
        let usable = context(&native, true);
        native.install_identity_first_context(usable);
        let accepted = native.identity_runtime().unwrap();
        let accepted_weak = accepted.downgrade();
        assert!(accepted_weak.upgrade().is_some());
        host.bind_identity_runtime(&mut native).await.unwrap();
        assert!(originals.bound_identity_runtime().is_ok());
        assert!(matches!(host.bind_identity_runtime(&mut native).await,
            Err(GatewayHostError::Denied)));

        // A later unusable candidate is still a conflicting bind. It cannot
        // overwrite the accepted owner or turn that conflict into Unavailable.
        let absent = context(&native, false);
        native.install_identity_first_context(absent);
        assert!(matches!(host.bind_identity_runtime(&mut native).await,
            Err(GatewayHostError::Denied)));
        assert!(originals.bound_identity_runtime().is_ok());
        assert!(owner.validate_current_selection().is_ok());
        assert_eq!(store.list_records(&scope, None, 32).await.unwrap(), policy_rows,
            "identity binding preserves the exact selected service policy row");
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grant_bytes);

        // No bootstrap or supervisor is started for these empty contexts.
        // The ordinary native shutdown below also checks per-use owner loss.
        drop(accepted);
    }).catch_unwind().await;

    // Retain and join the existing native cleanup even when an oracle panics.
    let closing = host.begin_close(GatewayHostCloseReason::HostDisconnected);
    let report = native.shutdown().await;
    let joined = host.join_close().await;
    assert!(closing.is_ok() && joined.is_ok());
    assert!(report.cleanup_completed());
    assert!(match report.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    });
    // A successful bind is no promise of future owner availability. The
    // existing per-use native upgrade must still refuse after owner shutdown.
    assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
    if let Err(payload) = checked { std::panic::resume_unwind(payload); }
}


// These four controls call the actual ordinary/prepared identity hooks. The
// existing empty native mob has no spawned member, model request or human login.
// Valid test policy is installed directly, without file-loader bootstrap.
fn selected_access_controller() -> meerkat_mobkit::access::AccessController {
    use meerkat_mobkit::access::{AccessControlConfig, AccessController, AccessGroup, AccessRule};
    AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["hook-fixture-admin".into()],
        groups: [("hook-viewers".into(), AccessGroup {
            members: vec!["hook-fixture-viewer".into()], ..Default::default()
        })].into(),
        rules: vec![AccessRule {
            id: "hook-view".into(), groups: vec!["hook-viewers".into()],
            actions: vec!["agent.view".into()], agents: vec!["*".into()],
            ..Default::default()
        }],
        ..Default::default()
    }).unwrap()
}

fn assert_hook_joined_source_publication(originals: &PrivateOriginals, native: &UnifiedRuntime) {
    use meerkat_authorization::publication::{LocalAuthorizationPublication, PublicationError};
    use meerkat_mobkit::access::AccessGroup;
    let access = native.access_controller().unwrap();
    let publication = originals.authorization_publication();
    let (before, stamp) = publication.observe(|| access.view_for_subject(Some("hook-fixture-viewer"))).unwrap();
    let unrelated = LocalAuthorizationPublication::new();
    let (_, unrelated_stamp) = unrelated.observe(|| ()).unwrap();
    assert!(before.enforced() && before.can_view_agent("agent"));
    stamp.check_current().unwrap();
    access.set_group("hook-viewers", AccessGroup::default()).unwrap();
    assert!(matches!(stamp.check_current(), Err(PublicationError::Changed)));
    let (after, fresh) = publication.observe(|| access.view_for_subject(Some("hook-fixture-viewer"))).unwrap();
    assert!(after.enforced() && !after.can_view_agent("agent"));
    fresh.check_current().unwrap();
    unrelated_stamp.check_current().unwrap();
}

async fn ordinary_access_hook_control(prejoined: bool) {
    use meerkat_authorization::publication::{LocalAuthorizationPublication, PublicationError};
    use meerkat_mobkit::access::AccessGroup;
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let (owner, installed) = InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
        .await.unwrap();
    let directory = state_directory(&selection, &installed.native, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let originals = installed.originals.clone();
    let store = installed.store.clone();
    let scope = installed.source_scope.clone();
    let policy_rows = store.list_records(&scope, None, 32).await.unwrap();
    assert_eq!(policy_rows.len(), 1);
    assert_eq!(policy_rows[0].data.schema, "toolkit.application-service-tool-policy");
    let (signal, logins) = crate::private_host::ConsoleLogins::fixture_owner();
    let host = installed.compose(Arc::new(owner), PrivateHostClosed(signal.subscribe()))
        .with_console_logins(logins).unwrap();
    let configuration = host.native_grant_work_configuration().unwrap().unwrap();
    let machine = Arc::new(MeerkatMachine::ephemeral()
        .with_local_grant_authorization(configuration).unwrap());
    let service = Arc::new(EphemeralSessionService::new(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()), 1,
    ));
    let spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
        .with_session_runtime_adapter(machine).unwrap()
        .with_declared_ephemeral_mob_storage();
    let spec = host.prepare_bootstrap(spec).await.unwrap();
    let mut native = UnifiedRuntime::bootstrap(spec, MetaConfig {
        modules: Vec::new(),
        discovery: DiscoverySpec { namespace: "factory".into(), modules: Vec::new() },
        pre_spawn: Vec::new(),
    }, Duration::from_secs(10)).await.unwrap();

    let checked = std::panic::AssertUnwindSafe(async {
        host.bind_runtime(&mut native).await.unwrap();
        native.install_identity_first_context(context(&native, true));
        let identity = native.identity_runtime().unwrap();
        assert!(identity.downgrade().upgrade().is_some());
        assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
        let access = selected_access_controller();
        let foreign = LocalAuthorizationPublication::new();
        if prejoined { access.join_authorization_publication(foreign.clone()).unwrap(); }
        native.set_access_controller(access);
        // The source task has not started before identity binding. A rejected
        // hook adds no owner; a successful hook may retain the same source.
        let before_ingress = Arc::strong_count(&originals);
        if prejoined {
            let (_, source_stamp) = originals.authorization_publication().observe(|| ()).unwrap();
            let (_, foreign_stamp) = foreign.observe(|| ()).unwrap();
            assert!(matches!(host.bind_identity_runtime(&mut native).await, Err(GatewayHostError::Denied)));
            assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
            assert_eq!(Arc::strong_count(&originals), before_ingress);
            native.access_controller().unwrap().set_group("hook-viewers", AccessGroup::default()).unwrap();
            assert!(matches!(foreign_stamp.check_current(), Err(PublicationError::Changed)));
            source_stamp.check_current().unwrap();
            // The same host, native identity and source remain selected. Only
            // the rejected controller is replaced; no owner is reconstructed.
            native.set_access_controller(selected_access_controller());
        }
        host.bind_identity_runtime(&mut native).await.unwrap();
        assert!(originals.bound_identity_runtime().is_ok());
        // The retained recovery task may also hold this same source during a round.
        assert!(Arc::strong_count(&originals) >= before_ingress + 1);
        assert_hook_joined_source_publication(&originals, &native);
        assert_eq!(store.list_records(&scope, None, 32).await.unwrap(), policy_rows,
            "identity binding preserves the exact selected service policy row");
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grant_bytes);
    }).catch_unwind().await;

    let closing = host.begin_close(GatewayHostCloseReason::HostDisconnected);
    let report = native.shutdown().await;
    let joined = host.join_close().await;
    assert!(closing.is_ok() && joined.is_ok());
    assert!(report.cleanup_completed());
    assert!(match report.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    });
    if let Err(payload) = checked { std::panic::resume_unwind(payload); }
}

#[tokio::test]
async fn ordinary_identity_hook_joins_present_controller_to_source_publication() {
    ordinary_access_hook_control(false).await;
}

#[tokio::test]
async fn ordinary_identity_hook_refuses_prejoined_controller_before_binding() {
    ordinary_access_hook_control(true).await;
}

// The real prepared registration relay needs its preparation participant.
// This empty-roster control never constructs a member or compiles a surface.
// Every collaborator method fails if that boundary is accidentally crossed.
struct UnenteredPreparationRegistration;
#[async_trait::async_trait]
impl meerkat_mob::MemberRuntimeRegistrationHook for UnenteredPreparationRegistration {
    async fn select_destination(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity)
        -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError>
    { panic!("empty hook control must not select a member destination") }
    async fn expected_destination(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity)
        -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError>
    { panic!("empty hook control must not restore a member destination") }
    async fn select_agent_mandate(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity)
        -> Result<Option<meerkat_runtime::meerkat_machine::SessionAgentMandate>, meerkat_mob::MobError>
    { panic!("empty hook control must not select a member mandate") }
    async fn expected_agent_mandate(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity)
        -> Result<Option<meerkat_runtime::meerkat_machine::SessionAgentMandate>, meerkat_mob::MobError>
    { panic!("empty hook control must not restore a member mandate") }
    async fn prepare_member_surface(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity,
        _: &meerkat_core::PreparedToolSurface)
        -> Result<meerkat_core::ToolSurfaceSelection, meerkat_mob::MobError>
    { panic!("empty hook control must not prepare a member surface") }
    async fn verify_member_surface(&self, _: &meerkat_mob::MemberRuntimeBuildIdentity,
        _: &[meerkat_core::ToolDef]) -> Result<(), meerkat_mob::MobError>
    { panic!("empty hook control must not verify a member surface") }
}

async fn prepared_access_hook_control(prejoined: bool) {
    use meerkat_authorization::publication::{LocalAuthorizationPublication, PublicationError};
    use meerkat_mobkit::access::AccessGroup;
    use meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy;
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let local: CompiledLocalPolicy = serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let selected = local.native_host.as_ref().unwrap();
    let directory = state_directory(&selection, selected, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let policy = PreparedInstalledToolPolicy::from_canonical(
        &read_bundle_artifact(&selection, &selected.application_tool_policy).unwrap(),
    ).unwrap();
    let registry = policy.registry();
    let prepared = PreparedInstalledHost::open(selection.deployment_root.clone(), PreparedLocalPolicy {
        deployment: local.deployment.clone(), components: local.components.clone(),
        channels: local.channels.clone(), native_host: selected.preparation(),
    }, operator_setup::OperatorOwnerStartup::Restore, policy).await.unwrap();
    let originals = prepared.originals();
    let registration = Arc::new(prepared_registration::PreparedMemberRegistration::new(
        Arc::new(UnenteredPreparationRegistration), registry,
        Arc::new(|_, _| panic!("empty hook control must not select a prepared destination")),
    ));
    let tools = acquisition::InstalledAcquisitionTools::new(Duration::from_secs(10)).unwrap();
    let participant = prepared.gateway_runtime(registration, tools,
        [toolkit_package::Id::new("hook-fixture-viewer").unwrap()].into()).unwrap();
    // HostJobSource requires the same real runtime store/jobs as this service.
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::SqliteSessionStore::open(directory.join("prepared-native-sessions.sqlite3")).unwrap()),
        runtime_store.clone(), Arc::new(meerkat::MemoryBlobStore::new()),
        prepared.take_configuration().unwrap(),
    ).unwrap();
    let jobs = persistence.job_store();
    let (service, machine) = meerkat::surface::build_runtime_backed_service(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()),
        1, persistence,
    ).unwrap();
    let service = Arc::new(service);
    let spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
        .with_session_runtime_adapter(machine).unwrap().with_declared_ephemeral_mob_storage()
        .with_runtime_delivery(meerkat_mobkit::mob_handle_runtime::MobRuntimeDelivery::new(runtime_store, jobs));
    let spec = participant.prepare_bootstrap(spec).unwrap();
    let mut native = UnifiedRuntime::bootstrap(spec, MetaConfig {
        modules: Vec::new(),
        discovery: DiscoverySpec { namespace: "factory".into(), modules: Vec::new() },
        pre_spawn: Vec::new(),
    }, Duration::from_secs(10)).await.unwrap();

    let checked = std::panic::AssertUnwindSafe(async {
        // This component control enters the real identity hook after actual
        // prepare_bootstrap. It does not enter bind_runtime's independent
        // continuation-service binding or claim complete prepared startup.
        native.install_identity_first_context(context(&native, true));
        let identity = native.identity_runtime().unwrap();
        assert!(identity.downgrade().upgrade().is_some());
        assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
        let access = selected_access_controller();
        let foreign = LocalAuthorizationPublication::new();
        if prejoined { access.join_authorization_publication(foreign.clone()).unwrap(); }
        native.set_access_controller(access);
        let before_ingress = participant.console_reference_count_for_test();
        if prejoined {
            let (_, source_stamp) = originals.authorization_publication().observe(|| ()).unwrap();
            let (_, foreign_stamp) = foreign.observe(|| ()).unwrap();
            assert!(matches!(participant.bind_identity_runtime(&mut native), Err(GatewayHostError::Denied)));
            assert!(matches!(originals.bound_identity_runtime(), Err(PrivateOriginalError::Unavailable)));
            assert_eq!(participant.console_reference_count_for_test(), before_ingress);
            native.access_controller().unwrap().set_group("hook-viewers", AccessGroup::default()).unwrap();
            assert!(matches!(foreign_stamp.check_current(), Err(PublicationError::Changed)));
            source_stamp.check_current().unwrap();
            native.set_access_controller(selected_access_controller());
        }
        // Success after a first-call refusal proves both the identity and
        // prepared access OnceLocks remained unused, on this same participant.
        participant.bind_identity_runtime(&mut native).unwrap();
        assert!(originals.bound_identity_runtime().is_ok());
        assert!(participant.console_reference_count_for_test() > before_ingress);
        assert_hook_joined_source_publication(&originals, &native);
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grant_bytes);
    }).catch_unwind().await;

    // The preparation has not installed a host lifetime. Join the same actual
    // native runtime before releasing its retained preparation/store owners.
    participant.begin_custody_close();
    let report = native.shutdown().await;
    participant.join_custody_close().await.unwrap();
    assert!(report.cleanup_completed());
    assert!(match report.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    });
    if let Err(payload) = checked { std::panic::resume_unwind(payload); }
}

#[tokio::test]
async fn prepared_identity_hook_joins_present_controller_to_source_publication() {
    prepared_access_hook_control(false).await;
}

#[tokio::test]
async fn prepared_identity_hook_refuses_prejoined_controller_before_binding() {
    prepared_access_hook_control(true).await;
}

#[path = "identity_binding/custody_recovery_tests.rs"]
mod custody_recovery_tests;
