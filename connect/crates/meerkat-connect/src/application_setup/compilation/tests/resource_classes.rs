//! Resource boundary controls using real native construction identities and
//! the adopted installed host's own registration, which selects first. The
//! hook refuses after the assertions, before actor/tool construction or a turn.
//! These controls do not claim a published dynamic member or callback execution.
use super::*;
use crate::application_setup::resource::{SetupResourceProvider, SetupResourceSelection};
use meerkat_core::mcp_attachment::McpAttachmentRefusal;
use meerkat_mob::{
    MemberDestinationSelection, MemberRuntimeBuildIdentity, MemberRuntimeRegistrationHook, MobError,
};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError,
    policy::{CompiledMemberClass, CompiledMemberClassInstance},
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use toolkit_mobkit::{
    installed_host::{
        PreparedInstallationOwner,
        dynamic_members::{
            ApplicationMemberPolicyRequest, MemberClassAdmissionRequest, PreparedMemberClassTools,
        },
        operator_setup::OperatorOwnerStartup,
        tool_policy::{ApplicationToolPolicyRecoveryDecision, PreparedInstalledToolPolicy},
    },
    private_originals::{
        ApplicationServicePolicy, ApplicationServicePolicyDecision,
        ApplicationServicePolicyRequest, ApplicationServicePolicyReservation,
    },
};

struct ExactClassPolicy {
    owner: meerkat_core::PrincipalRef,
    class: CompiledMemberClass,
    instance: CompiledMemberClassInstance,
    bootstrap: Vec<u8>,
    current: Arc<AtomicBool>,
}
struct Reservation {
    owner: meerkat_core::PrincipalRef,
    current: Arc<AtomicBool>,
    provenance: serde_json::Map<String, serde_json::Value>,
}
impl ApplicationServicePolicyReservation for Reservation {
    fn owner(&self) -> &meerkat_core::PrincipalRef {
        &self.owner
    }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}
impl ApplicationServicePolicy for ExactClassPolicy {
    fn reserve(
        &self,
        _: &ApplicationServicePolicyRequest<'_>,
    ) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(ApplicationServicePolicyDecision::RequiresHuman)
    }
    fn reserve_member_policy(
        &self,
        request: &ApplicationMemberPolicyRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        if request.owner != &self.owner
            || request.realm != &meerkat_core::mob_realm_id("household").unwrap()
            || request.deployment != "compiler-fixture"
            || request.admission.class != self.class
            || request.admission.instance != self.instance
            || request.canonical_policy != self.bootstrap
            || !self.current.load(Ordering::SeqCst)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            Reservation {
                owner: self.owner.clone(),
                current: self.current.clone(),
                provenance: Default::default(),
            },
        )))
    }
}
struct EmptyClassFactory;
#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for EmptyClassFactory {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Vec::new().into()
    }
    async fn dispatch(
        &self,
        _: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        panic!("the destination control must not enter a tool")
    }
}

struct ResourceHook {
    // The adopted installed host's own registration (forwarded by the prepared
    // registration it adopted). It selects each destination before the checks.
    installed: Arc<dyn MemberRuntimeRegistrationHook>,
    resource: Arc<SetupResourceSelection>,
    policy: PreparedInstalledToolPolicy,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    instance: CompiledMemberClassInstance,
    current: Arc<AtomicBool>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl MemberRuntimeRegistrationHook for ResourceHook {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        let session: meerkat_core::SessionId =
            meerkat_core::SessionId::parse(identity.session_document().as_str()).unwrap();
        let bindings = self
            .machine
            .prepare_bindings(session.clone())
            .await
            .unwrap();
        let authority = bindings.configured_mcp_attachment().cloned().unwrap();
        let expected = MemberDestinationSelection {
            logical_owner: self.instance.member.logical_owner.clone(),
            context: identity.session_document().clone(),
            context_generation: self.instance.member.context_generation,
            audience: self.instance.member.audience.clone(),
        };
        // The real installed registration selects first: for the admitted
        // instance it records the class registration the checks below read,
        // and it refuses any member it never admitted.
        let installed = self.installed.select_destination(identity).await;
        if identity.member() == &self.instance.member.member {
            assert_eq!(
                installed.unwrap(),
                expected,
                "the installed hook selects the exact admitted destination"
            );
            assert_eq!(
                self.policy.member_class_destination(identity).unwrap(),
                Some(expected.clone())
            );
            for changed in [
                MemberDestinationSelection {
                    context: meerkat_authorization_contracts::evidence::EvidenceId::new(
                        "another-real-document",
                    )
                    .unwrap(),
                    ..expected.clone()
                },
                MemberDestinationSelection {
                    context_generation: expected.context_generation + 1,
                    ..expected.clone()
                },
                MemberDestinationSelection {
                    audience:
                        meerkat_authorization_contracts::constraints::AudienceRef::Destination {
                            authority: expected.logical_owner.clone(),
                            destination_id: "another-project".into(),
                        },
                    ..expected.clone()
                },
            ] {
                assert!(
                    self.resource
                        .selected_destination(identity, &changed)
                        .is_err()
                );
            }
            self.resource
                .selected_destination(identity, &expected)
                .unwrap();
            assert_eq!(
                authority.authorize_configured(self.resource.server()),
                Err(McpAttachmentRefusal::NotCurrent)
            );
            assert!(self.resource.registration_slots.lock().unwrap().is_empty());
            self.current.store(false, Ordering::SeqCst);
            assert!(
                self.resource
                    .selected_destination(identity, &expected)
                    .is_err(),
                "revoked canonical admission cannot be reused"
            );
        } else {
            assert!(
                installed.is_err(),
                "the installed hook refuses a member it never admitted"
            );
            // The same packaged profile is not an admitted concrete member.
            assert!(
                self.resource
                    .selected_destination(identity, &expected)
                    .is_err()
            );
            assert_eq!(
                authority.authorize_configured(self.resource.server()),
                Err(McpAttachmentRefusal::NotCurrent)
            );
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(MobError::WiringError(
            "resource control stops before native actor construction".into(),
        ))
    }
    async fn expected_destination(
        &self,
        _: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        panic!("this control never resumes an actor")
    }
    async fn verify_member_surface(
        &self,
        _: &MemberRuntimeBuildIdentity,
        _: &[ToolDef],
    ) -> Result<(), MobError> {
        panic!("this control stops before surface construction")
    }
}

#[test]
fn class_resource_requires_exact_native_admission_and_never_selects_helper() {
    // The SDK parent's private-host pipes exist before the test runtime.
    let (pipes, parent) = crate::resources::sdk_host::SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(class_resource_control(pipes, parent));
}

async fn class_resource_control(
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    parent: crate::resources::sdk_host::SdkParent,
) {
    use futures_util::FutureExt;
    use meerkat_toolkit::host_lifetime::control::{HostCloseState, PrivateHostLifetimeControl};
    use toolkit_mobkit::{
        installed_host::{
            preparation::PreparationAuthorities, prepared_registration::PreparedMemberRegistration,
        },
        private_host::{PrivateHostConnection, PrivateHostStartup},
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let input = with_uninstantiated_class(&root);
    let phase = Arc::new(
        input
            .validate()
            .unwrap()
            .prepare(super::super::super::tests::prepared(&root))
            .unwrap(),
    );
    // Like the other registration fixtures, the native owner's operator is a
    // service account distinct from the grant administrator that owns the
    // class policy: the selection refuses an operator equal to it.
    let operator = meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount,
        "class-registration-admin",
        meerkat_core::TrustDomainId::new("application-setup-fixture").unwrap(),
    )
    .unwrap();
    let selected = owner_selection(&phase, Some(operator));
    let class = phase.input.member_classes[0].clone();
    let mut member = phase.input.members[0].clone();
    member.member.member = "project-first".into();
    member.member.role = class.profile.clone();
    member.setup_capability = None;
    let instance = CompiledMemberClassInstance {
        class: class.id.clone(),
        member,
    };
    let current = Arc::new(AtomicBool::new(true));
    let participant = Arc::new(ExactClassPolicy {
        owner: selected.native_host.grant_administrator.clone(),
        class: class.clone(),
        instance: instance.clone(),
        bootstrap: phase.policy_bytes.clone(),
        current: current.clone(),
    });
    std::fs::create_dir_all(root.join("shared/data")).unwrap();
    let mut prepared = PreparedInstallationOwner::open(
        root.clone(),
        selected,
        OperatorOwnerStartup::Initialize {
            namespace: meerkat_authorization_contracts::evidence::EvidenceId::new(
                "resource-classes",
            )
            .unwrap(),
            generation: 1,
        },
    )
    .unwrap()
    .prepare_with_application_service_policy(phase.prepared_tool_policy(), participant)
    .await
    .unwrap();
    let policy = phase.prepared_tool_policy();
    assert_eq!(
        policy.member_class_registration_capacity().unwrap(),
        0,
        "no dummy class instance is registered"
    );
    let admitted = policy
        .admit_member_class(MemberClassAdmissionRequest {
            id: toolkit_package::Id::new("project-first-admission").unwrap(),
            instance: instance.clone(),
            tools: PreparedMemberClassTools::from_factory(
                &class,
                &class.factory,
                Arc::new(EmptyClassFactory),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(policy.member_class_registration_capacity().unwrap(), 1);
    let mut configuration = prepared.take_configuration().unwrap();
    configuration.configured_mcp_attachment = Some(Arc::new(SetupResourceProvider::new(
        phase.resource_selection().unwrap(),
        configuration.configured_mcp_attachment.take(),
    )));
    // As the driver does, the prepared owner finalizes its authorities (this
    // control issues no helper grants), the compiler retains exactly that
    // owner with both compiled members' surfaces, and its completed bundle is
    // published as the current activation the installed owner adopts.
    let finalized = prepared
        .finalize_authorities_without_helper(&PreparationAuthorities::default())
        .unwrap();
    phase.capture(0, &surface(false)).unwrap();
    phase.capture(1, &surface(true)).unwrap();
    phase.retain_owner(finalized).unwrap();
    let (completed, _) = phase.complete_bundle(&root.join("complete")).unwrap();
    let selection =
        super::publish_completed(&root, "classes", &completed, &root.join("complete"), false);
    let registration = Arc::new(PreparedMemberRegistration::new(
        phase.clone(),
        phase.registry(),
        {
            let phase = phase.clone();
            Arc::new(move |identity, destination| phase.selected_destination(identity, destination))
        },
    ));
    let prepared_policy = prepared.policy();
    // The supported private-host bootstrap adopts the published selection and
    // composes the installed host with the prepared registration, which then
    // forwards to the installed host's own registration hook.
    let client = parent.enter();
    let lifetime = Arc::new(std::sync::Mutex::new(None));
    let adopted = Arc::new(std::sync::Mutex::new(None));
    let server = {
        let lifetime = lifetime.clone();
        let saved = adopted.clone();
        let registration = registration.clone();
        async move {
            let startup = PrivateHostStartup::read(pipes).await?;
            PrivateHostConnection::bootstrap_prepared_with_controller_reviews(
                startup,
                move |selection| async move {
                    let (owner, installed) = prepared.adopt(selection)?;
                    let originals = installed.originals();
                    *adopted.lock().unwrap() = Some(installed);
                    Ok((owner, originals))
                },
                move |owner, closed, logins| async move {
                    *lifetime.lock().unwrap() = Some(owner.clone());
                    let installed = saved
                        .lock()
                        .unwrap()
                        .take()
                        .ok_or(HostLifetimeError::Unavailable)?;
                    installed
                        .compose_with_prepared_registration(
                            owner,
                            closed,
                            registration,
                            prepared_policy,
                        )?
                        .with_console_logins(logins)
                },
            )
            .await
        }
    };
    let request = {
        let client = client.clone();
        async move {
            let mut client = client.lock().await;
            let reply = client
                .call(serde_json::json!({"kind":"bootstrap", "selection":selection}))
                .await;
            if reply.is_err() {
                let _ = client.close().await;
            }
            reply
        }
    };
    let (reply, server) =
        tokio::join!(request, std::panic::AssertUnwindSafe(server).catch_unwind());
    let (host, connection, local, remote) = match server {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => panic!("private bootstrap refused: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    };
    drop(local);
    drop(remote);
    assert_eq!(reply.unwrap()["kind"], "bootstrap_accepted");
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::SqliteSessionStore::open(root.join("sessions.sqlite3")).unwrap()),
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat::MemoryBlobStore::new()),
        configuration,
    )
    .unwrap();
    let (service, machine) = meerkat::surface::build_runtime_backed_service(
        meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::minimal().comms(true),
            meerkat::Config::default(),
        ),
        2,
        persistence,
    )
    .unwrap();
    let hook = Arc::new(ResourceHook {
        installed: registration,
        resource: phase.resource_selection().unwrap(),
        policy,
        machine: machine.clone(),
        instance,
        current,
        calls: AtomicUsize::new(0),
    });
    // The canonical admitted instance adds one pending-correlation slot, not
    // an unlimited class capacity. Refusal leaves no usable setup authority.
    let capacity = phase.input.members.len() + 1;
    let mut pending = Vec::new();
    for index in 0..=capacity {
        let session = meerkat_core::SessionId::new();
        machine.register_session(session.clone()).await.unwrap();
        let bindings = machine.prepare_bindings(session.clone()).await.unwrap();
        assert_eq!(
            bindings
                .configured_mcp_attachment()
                .unwrap()
                .authorize_configured(phase.server().unwrap()),
            Err(McpAttachmentRefusal::NotCurrent)
        );
        assert_eq!(
            phase
                .resource
                .as_ref()
                .unwrap()
                .registration_slots
                .lock()
                .unwrap()
                .contains_key(&session.to_string()),
            index < capacity
        );
        pending.push((session, bindings));
    }
    assert_eq!(
        phase
            .resource
            .as_ref()
            .unwrap()
            .registration_slots
            .lock()
            .unwrap()
            .len(),
        capacity
    );
    for (session, _) in &pending {
        machine.unregister_session(session).await.unwrap();
    }
    drop(pending);
    // The compiler's historical data fixture names a `research` skill that it
    // never defines, a non-executable placeholder. Native mob construction
    // resolves every profile's skill references, so this control defines that
    // one skill; the compiled profiles, members and resources are unchanged.
    let mut definition = phase.definition().clone();
    definition.skills.insert(
        "research".into(),
        meerkat_mob::definition::SkillSource::Inline {
            content: "Research placeholder for the resource class control.".into(),
        },
    );
    let handle = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
        .with_session_service(Arc::new(service))
        .with_runtime_adapter(machine)
        .with_member_runtime_registration_hook(hook.clone())
        .create()
        .await
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(async {
        for name in ["same-profile-not-admitted", "project-first"] {
            assert!(
                handle
                    .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                        class.profile.as_str(),
                        meerkat_mob::AgentIdentity::from(name)
                    ))
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            hook.calls.load(Ordering::SeqCst),
            2,
            "both assertions ran inside actual native destination hooks"
        );
    })
    .catch_unwind()
    .await;
    let stopped = handle.shutdown().await;
    drop(admitted);
    // Close the SDK parent, its private host connection and the adopted
    // lifetime in the order every private-host fixture uses.
    let mut cleanup = Vec::new();
    if let Err(error) = client.lock().await.close().await {
        cleanup.push(error);
    }
    if let Err(error) = connection.finish().await {
        cleanup.push(error.to_string());
    }
    let owner = lifetime.lock().unwrap().take();
    match owner {
        Some(owner) => match owner.host_lost() {
            Ok(close) => match owner.join_close(&close.handle).await {
                Ok(value) if value.state == HostCloseState::Settled => {}
                Ok(_) => cleanup.push("private lifetime close remains unresolved".into()),
                Err(error) => cleanup.push(error.to_string()),
            },
            Err(error) => cleanup.push(error.to_string()),
        },
        None => cleanup.push("the adopted lifetime was not retained".into()),
    }
    drop(host);
    assert!(stopped.is_ok());
    assert!(
        cleanup.is_empty(),
        "the private host owners must be joined: {cleanup:?}"
    );
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}
