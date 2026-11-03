//! Resource boundary controls using real native construction identities. The
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
        if identity.member() == &self.instance.member.member {
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

#[tokio::test]
async fn class_resource_requires_exact_native_admission_and_never_selects_helper() {
    use futures_util::FutureExt;
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
    let prepared = PreparedInstallationOwner::open(
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
    assert!(stopped.is_ok());
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}
