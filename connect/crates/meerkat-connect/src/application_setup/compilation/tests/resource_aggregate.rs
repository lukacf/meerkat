//! Aggregate resource/registration controls. These use actual native session
//! registration and retained preparer descriptors. select_registration is the
//! existing mechanical destination-hook port, not actual member construction.
//! The shared SDK case separately exercises the real two-helper destination hook.
use super::*;
use crate::application_setup::resource::{SetupResourceProvider, SetupResourceSelection};
use meerkat_core::{
    SessionId,
    mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal},
};
use meerkat_runtime::meerkat_machine::{ConfiguredMcpAttachmentProvider, RuntimeSessionRegistrationWitness, SessionAgentBinding};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Inherited {
    configured: AtomicUsize,
    selections: AtomicUsize,
    calls: Arc<AtomicUsize>,
}
struct InheritedAuthority(Arc<AtomicUsize>);
impl ConfiguredMcpAttachmentAuthority for InheritedAuthority {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl ConfiguredMcpAttachmentProvider for Inherited {
    fn configured(
        &self,
        _: &RuntimeSessionRegistrationWitness,
        _: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        self.configured.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new(InheritedAuthority(self.calls.clone())))
    }
    fn agent_selection(
        &self,
        _: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        self.selections.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new(InheritedAuthority(self.calls.clone())))
    }
}

fn second_phase(root: &Path) -> ApplicationSetupCompilation {
    let mut input = selected(root);
    let identity = "helper:second";
    let member_id = roster_member_id_for_identity(identity).to_string();
    input.roster[1].identity =
        meerkat_mobkit::identity_first::AgentIdentity::parse(identity).unwrap();
    input.members[1].member.member = member_id.clone();
    input.helper.member.member = member_id.clone();
    input.tool_policy.members[1].member_identity = member_id;
    input
        .tool_policy
        .members
        .sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
    let handoff = toolkit_package::canonical_json(&json!({"mcpServers":{"toolkit-setup":{
        "command":"/usr/bin/python3", "args":["-I","-B","/retained/second-adapter.py"]
    }}}))
    .unwrap();
    let artifact_directory = root.join("retained-preparer");
    std::fs::create_dir(&artifact_directory).unwrap();
    std::fs::write(artifact_directory.join("prepare.stdout"), &handoff).unwrap();
    input
        .validate()
        .unwrap()
        .prepare(PreparedResources {
            artifact_directory,
            connection: PreparedSetupConnection::from_preparer_output(&handoff).unwrap(),
        })
        .unwrap()
}

async fn aggregate_fixture(
    root: &Path,
) -> (
    [Arc<SetupResourceSelection>; 2],
    Arc<meerkat_runtime::MeerkatMachine>,
    toolkit_mobkit::installed_host::PreparedInstalledHost,
    Arc<Inherited>,
    Arc<SetupResourceProvider>,
) {
    use toolkit_mobkit::installed_host::{
        PreparedInstallationOwner, operator_setup::OperatorOwnerStartup,
    };
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let a = phase(&a);
    let b = second_phase(&b);
    assert_ne!(a.helper().member, b.helper().member);
    assert_eq!(a.server().name, b.server().name);
    assert_ne!(a.server(), b.server());
    let mut selected = a.final_authorities.get().unwrap().owner.clone();
    selected.native_host.remote_operator = Some(
        meerkat_core::PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            "registration-admin",
            meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
        )
        .unwrap(),
    );
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    let owner = PreparedInstallationOwner::open(
        root.to_owned(),
        selected,
        OperatorOwnerStartup::fresh("aggregate-registration", 1).unwrap(),
    )
    .unwrap()
    .complete(a.prepared_tool_policy())
    .await
    .unwrap();
    let resources = [a.resource_selection(), b.resource_selection()];
    let inherited = Arc::new(Inherited::default());
    let provider = Arc::new(SetupResourceProvider::multiple(
        resources.to_vec(),
        Some(inherited.clone()),
    ));
    let mut configuration = owner.take_configuration().unwrap();
    assert!(configuration.configured_mcp_attachment.is_none());
    configuration.configured_mcp_attachment = Some(provider.clone());
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .unwrap(),
    );
    (resources, machine, owner, inherited, provider)
}

fn select(
    resources: &[Arc<SetupResourceSelection>; 2],
    session: &SessionId,
    helper: Option<usize>,
) {
    let document =
        meerkat_authorization_contracts::evidence::EvidenceId::new(session.to_string()).unwrap();
    for (index, resource) in resources.iter().enumerate() {
        resource
            .select_registration(&document, helper == Some(index))
            .unwrap();
    }
}

#[tokio::test]
async fn aggregate_resources_keep_same_name_destinations_and_delegation_exact() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let (resources, machine, _owner, inherited, provider) = aggregate_fixture(&root).await;
    let sessions = [SessionId::new(), SessionId::new(), SessionId::new()];
    let mut authorities = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        machine.register_session(session.clone()).await.unwrap();
        let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
        let authority = bindings.configured_mcp_attachment().cloned().unwrap();
        for resource in &resources {
            assert_eq!(
                authority.authorize_configured(resource.server()),
                Err(McpAttachmentRefusal::NotCurrent)
            );
        }
        // Exercise agent-selection forwarding with an actual retained native
        // registration, before this mechanical selected-destination port.
        if index == 0 {
            let participant = resources[0]
                .registration_slots
                .lock()
                .unwrap()
                .get(&session.to_string())
                .unwrap()
                .upgrade()
                .unwrap();
            let delegated = provider.agent_selection(participant.agent.clone()).unwrap();
            let mut unrelated = resources[0].server().clone();
            unrelated.name = "business-service".into();
            delegated.authorize_configured(&unrelated).unwrap();
        }
        select(&resources, session, (index < 2).then_some(index));
        authorities.push(authority);
    }
    assert_eq!(inherited.configured.load(Ordering::SeqCst), 3);
    assert_eq!(inherited.selections.load(Ordering::SeqCst), 1);
    assert_eq!(inherited.calls.load(Ordering::SeqCst), 1);
    for (index, authority) in authorities.iter().enumerate() {
        for (peer, resource) in resources.iter().enumerate() {
            assert_eq!(
                authority.authorize_configured(resource.server()),
                if index == peer {
                    Ok(())
                } else {
                    Err(McpAttachmentRefusal::NotCurrent)
                }
            );
        }
        let mut wrong = resources[0].server().clone();
        wrong.access = None;
        assert_eq!(
            authority.authorize_configured(&wrong),
            Err(McpAttachmentRefusal::Mismatch)
        );
        wrong = resources[1].server().clone();
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(ref mut transport) =
            wrong.transport
        else {
            panic!("retained stdio resource");
        };
        transport.args.push("foreign-peer".into());
        assert_eq!(
            authority.authorize_configured(&wrong),
            Err(McpAttachmentRefusal::Mismatch)
        );
    }
    assert_eq!(
        inherited.calls.load(Ordering::SeqCst),
        1,
        "known names never reach permissive inherited authority"
    );
    let mut unrelated = resources[0].server().clone();
    unrelated.name = "business-service".into();
    authorities[0].authorize_configured(&unrelated).unwrap();
    assert_eq!(inherited.calls.load(Ordering::SeqCst), 2);
    for session in sessions {
        machine.unregister_session(&session).await.unwrap();
    }
}

#[tokio::test]
async fn aggregate_resource_retirement_and_replacement_require_new_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let (resources, machine, _owner, inherited, _provider) = aggregate_fixture(&root).await;
    let a = SessionId::new();
    let b = SessionId::new();
    machine.register_session(a.clone()).await.unwrap();
    machine.register_session(b.clone()).await.unwrap();
    let a_bindings = machine.prepare_bindings(a.clone()).await.unwrap();
    let b_bindings = machine.prepare_bindings(b.clone()).await.unwrap();
    let old_a = a_bindings.configured_mcp_attachment().cloned().unwrap();
    let current_b = b_bindings.configured_mcp_attachment().cloned().unwrap();
    select(&resources, &a, Some(0));
    select(&resources, &b, Some(1));
    old_a.authorize_configured(resources[0].server()).unwrap();
    current_b
        .authorize_configured(resources[1].server())
        .unwrap();
    machine.unregister_session(&a).await.unwrap();
    machine.register_session(a.clone()).await.unwrap();
    let replacement = machine.prepare_bindings(a.clone()).await.unwrap();
    let new_a = replacement.configured_mcp_attachment().cloned().unwrap();
    assert_eq!(
        old_a.authorize_configured(resources[0].server()),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert_eq!(
        new_a.authorize_configured(resources[0].server()),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    current_b
        .authorize_configured(resources[1].server())
        .unwrap();
    select(&resources, &a, Some(0));
    new_a.authorize_configured(resources[0].server()).unwrap();
    assert_eq!(
        old_a.authorize_configured(resources[0].server()),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert_eq!(
        new_a.authorize_configured(resources[1].server()),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert_eq!(inherited.calls.load(Ordering::SeqCst), 0);
    machine.unregister_session(&a).await.unwrap();
    machine.unregister_session(&b).await.unwrap();
}
