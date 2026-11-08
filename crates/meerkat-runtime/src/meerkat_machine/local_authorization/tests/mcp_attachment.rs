//! Governed MCP attachment control on the actual native owners: prepared once
//! under the bound destination, rechecked at each effect entry, entered once,
//! and never stretched from a live attachment to a configuration write.

use super::*;
use crate::meerkat_machine::RuntimeSessionRegistrationWitness;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::mcp_attachment::McpAttachmentRefusal;
use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentFacts};
use meerkat_core::mcp_config::{McpScope, McpServerConfig};
use std::sync::atomic::{AtomicBool, Ordering};

/// The trusted ingress receipt the owners verify.
struct AttachmentReceipt;

/// The installed mandate owner, plus the attachment mandate it resolves.
struct Mandate {
    inner: Arc<dyn AdmittedWorkPolicyOwner>,
    requester: PrincipalRef,
}

impl AdmittedWorkPolicyOwner for Mandate {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner
            .authorize_admitted_work(association, binding, purpose, now_ms)
    }

    fn authorize_mcp_attachment(
        &self,
        control: &McpAttachmentControlRequest,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if control.evidence::<AttachmentReceipt>().is_none()
            || control.facts().requester != self.requester
        {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
        })
    }
}

/// The installed operation owner, plus current server/configuration rules.
struct Servers {
    inner: Arc<dyn OperationPolicyOwner>,
    domain: ResourceDomain,
    processor: PrincipalRef,
    server_allowed: AtomicBool,
    persist_allowed: AtomicBool,
    review_required: AtomicBool,
}

impl OperationPolicyOwner for Servers {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        self.inner
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.inner
            .authorize_operation(association, binding, purpose, now_ms)
    }

    fn authorize_mcp_attachment(
        &self,
        control: &McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let allowed = match &binding.facts().operation {
            AuthorizationOperation::Source(source) => match source.usage {
                SourceAuthorizationUse::Hydrate => self.server_allowed.load(Ordering::SeqCst),
                SourceAuthorizationUse::Retain => self.persist_allowed.load(Ordering::SeqCst),
                SourceAuthorizationUse::Read => false,
            },
            AuthorizationOperation::Publication(_) => true,
            _ => false,
        };
        if !allowed || control.server().name != "records" {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: ActionRef {
                    feature: "native-test".into(),
                    action: "attach".into(),
                },
                resource_domain: self.domain.clone(),
                processor: ProcessorRef::Principal {
                    principal: self.processor.clone(),
                },
                audience: AudienceRef::Principal {
                    principal: self.processor.clone(),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
            review_tier: if self.review_required.load(Ordering::SeqCst) {
                meerkat_core::authorization::OperationReviewTier::R2
            } else {
                meerkat_core::authorization::OperationReviewTier::R1
            },
        })
    }
}

struct IdleExecutor;

#[async_trait::async_trait]
impl CoreExecutor for IdleExecutor {
    async fn apply(
        &mut self,
        _: meerkat_core::RunId,
        _: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        Err(CoreExecutorError::Stopped)
    }
    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct Fixture {
    machine: Arc<MeerkatMachine>,
    session: SessionId,
    registration: RuntimeSessionRegistrationWitness,
    target: NativeWorkTarget,
    servers: Arc<Servers>,
    publication: LocalAuthorizationPublication,
    requester: PrincipalRef,
    grants: Arc<meerkat_authorization::grants::LocalGrantAuthority>,
    root: PrincipalRef,
    executor: PrincipalRef,
}

/// The host composition's own installation decision: every server.
struct AllInstalled;
impl meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority for AllInstalled {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        Ok(())
    }
}

impl Fixture {
    async fn new() -> Self {
        Self::build(false, None, false).await
    }

    /// The same owners behind the per-agent host: attachments are decided
    /// for the registration's bound agent and the control's server.
    async fn per_agent() -> Self {
        Self::build(false, Some(Arc::new(Clock)), false).await
    }

    /// The same owners behind the GENERIC native/grant host, whose per-agent
    /// configured provider shares one agent grant owner between configured
    /// servers and attachment controls.
    async fn generic_per_agent() -> Self {
        Self::build(false, Some(Arc::new(Clock)), true).await
    }

    /// The same owners installed through the production scoped original
    /// composition (which requires scoped destinations).
    async fn scoped() -> Self {
        Self::build(true, None, false).await
    }

    /// With `agent_clock`, the owners sit behind a per-agent host whose
    /// agent grant reads that clock: the specialized per-agent composition,
    /// or with `generic` the generic native/grant host and its per-agent
    /// configured provider.
    async fn build(
        scoped: bool,
        agent_clock: Option<Arc<dyn meerkat_authorization::clock::LocalAuthorizationClock>>,
        generic: bool,
    ) -> Self {
        let (mut configuration, prompt, account, publication) =
            mutable_controller_configuration(true);
        let candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        let requester = candidate.requester.clone();
        let grants = Arc::clone(&configuration.grants);
        let root = candidate.controller_grant_lineage[0].root_authority.clone();
        let executor = candidate.logical_executor.clone();
        let servers = Arc::new(Servers {
            inner: account.clone(),
            domain: account.domain.clone(),
            processor: candidate.logical_executor.clone(),
            server_allowed: AtomicBool::new(true),
            persist_allowed: AtomicBool::new(false),
            review_required: AtomicBool::new(false),
        });
        let mandate = Arc::new(Mandate {
            inner: Arc::clone(&configuration.invocation_owner),
            requester: requester.clone(),
        });
        let configuration = if scoped {
            crate::meerkat_machine::ScopedGrantComposition {
                grants: Arc::clone(&configuration.grants),
                ingress_domain: candidate.ingress_namespace.ingress.clone(),
                originals: Arc::new(NoOriginals),
                invocation: crate::meerkat_machine::ScopedInvocation::from_policy(mandate),
                operation_owner: servers.clone(),
                retained_resume: None,
            }
            .into_configuration()
        } else {
            configuration.invocation_owner = mandate;
            configuration.operation_owner = servers.clone();
            configuration
        };
        let machine = Arc::new(if let (Some(agent_clock), true) = (&agent_clock, generic) {
            let mut configuration = configuration;
            configuration.configured_mcp_attachment = Some(Arc::new(
                crate::meerkat_machine::AgentConfiguredMcpProvider::new(
                    crate::meerkat_machine::AgentInstallationGrant::new(
                        Arc::clone(&grants),
                        Arc::clone(agent_clock),
                        root.clone(),
                    ),
                    Arc::new(AllInstalled),
                    None,
                ),
            ));
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization(configuration)
                .expect("actual generic per-agent composition")
        } else if let Some(agent_clock) = agent_clock {
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization_and_agent_mcp(
                    configuration,
                    Arc::new(AllInstalled),
                    crate::meerkat_machine::AgentInstallationGrant::new(
                        Arc::clone(&grants),
                        agent_clock,
                        root.clone(),
                    ),
                    None,
                )
                .expect("actual per-agent composition")
        } else {
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization(configuration)
                .expect("actual native/grant composition")
        });
        let session = SessionId::new();
        machine
            .register_session(session.clone())
            .await
            .expect("actual registration");
        machine
            .prepare_bindings(session.clone())
            .await
            .expect("actual runtime binding");
        machine
            .register_session_with_executor(session.clone(), Box::new(IdleExecutor))
            .await
            .expect("actual executor attachment");
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .expect("current registration");
        let mut target = candidate.target.clone();
        target.logical_runtime =
            EvidenceId::new(MeerkatMachine::logical_runtime_id(&session).to_string())
                .expect("runtime");
        machine
            .bind_scoped_context(&registration, &target)
            .await
            .expect("bind the session's destination");
        Self {
            machine,
            session,
            registration,
            target,
            servers,
            publication,
            requester,
            grants,
            root,
            executor,
        }
    }

    /// A grant to the fixture's agent for the installation of `server`.
    fn issue(
        &self,
        id: &str,
        server: &str,
    ) -> meerkat_authorization_contracts::grant::GrantLineageRef {
        use meerkat_authorization_contracts::constraints::{DomainRestriction, ExactRestriction};
        self.grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &self.root,
                EvidenceId::new(id).expect("id"),
                self.executor.clone(),
                None,
                // The fixture's servers have no connector binding: the grant
                // approves exactly that, explicitly.
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([
                        ActionRef {
                            feature:
                                meerkat_authorization::mcp_installation::MCP_INSTALLATION_FEATURE
                                    .to_owned(),
                            action: server.to_owned(),
                        },
                        meerkat_authorization::mcp_installation::mcp_connector_action(
                            server,
                            &meerkat_core::McpConnectorApproval::NoConnector,
                        ),
                    ]),
                    resource_domains: DomainRestriction::exact([
                        ResourceDomain {
                            authority: self.root.clone(),
                            namespace: format!("mcp-server/{server}"),
                        },
                        meerkat_authorization::mcp_installation::mcp_connector_domain(
                            &self.root,
                            server,
                            &meerkat_core::McpConnectorApproval::NoConnector,
                        ),
                    ]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("agent grant")
    }

    /// Bind the agent with one selected lineage per listed server.
    async fn select(
        &self,
        servers: &[(
            &str,
            meerkat_authorization_contracts::grant::GrantLineageRef,
        )],
    ) {
        let servers = servers
            .iter()
            .map(|(server, grant)| ((*server).to_owned(), vec![grant.clone()]))
            .collect();
        self.machine
            .bind_session_agent_mandate(
                &self.registration,
                crate::meerkat_machine::SessionAgentMandate::new(
                    self.executor.clone(),
                    servers,
                    None,
                )
                .expect("mandate"),
            )
            .await
            .expect("bind the trusted selection");
    }

    async fn prepare(
        &self,
    ) -> Result<Arc<dyn meerkat_core::mcp_attachment::McpAttachmentAuthority>, McpAttachmentRefusal>
    {
        self.machine
            .prepare_mcp_attachment(&self.registration, &self.target, self.control())
            .await
    }

    fn control(&self) -> Arc<McpAttachmentControlRequest> {
        McpAttachmentControlRequest::from_trusted_ingress(
            self.session.clone(),
            McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
            McpAttachmentFacts {
                requester: self.requester.clone(),
                actor: self.requester.clone(),
                realm: meerkat_core::connection::RealmId::parse("native-test").expect("realm"),
                destination: crate::meerkat_machine::scoped_destination_target(&self.target),
            },
            Arc::new(AttachmentReceipt),
        )
        .expect("qualified control")
    }

    /// Change the owner's facts under one published owner change.
    fn change(&self, apply: impl FnOnce(&Servers)) {
        let guard = self.publication.begin_owner_change().expect("owner change");
        apply(&self.servers);
        drop(guard);
    }
}

/// A prepared attachment is applied once, after its native checks; the live
/// permit does not cover a configuration write, which its own owner decision
/// must allow.
#[tokio::test]
async fn a_prepared_attachment_applies_once_and_never_persists_by_itself() {
    let fixture = Fixture::new().await;
    let control = fixture.control();
    let attachment = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, Arc::clone(&control))
        .await
        .expect("prepared under the current owners");
    assert_eq!(attachment.server(), control.server());
    assert_eq!(
        attachment.authorize_persist(McpScope::Project),
        Err(McpAttachmentRefusal::Denied),
        "the live decision does not authorize configuration"
    );
    attachment.authorize_apply().expect("entered once");
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::NotCurrent),
        "a control is consumed by its one live attachment"
    );

    fixture.change(|servers| servers.persist_allowed.store(true, Ordering::SeqCst));
    let persisting = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("prepared");
    persisting
        .authorize_persist(McpScope::Project)
        .expect("the configuration owner allows this exact write");
    assert_eq!(
        persisting.authorize_persist(McpScope::Project),
        Err(McpAttachmentRefusal::NotCurrent),
        "one write per scope"
    );
}

/// Refusals are local and typed: a denied server never prepares; a grant
/// change after preparation refuses at entry; a required review refuses at
/// entry; a retired destination is no longer current.
#[tokio::test]
async fn attachment_refusals_are_rechecked_at_entry() {
    let fixture = Fixture::new().await;
    fixture.change(|servers| servers.server_allowed.store(false, Ordering::SeqCst));
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
            .await
            .err(),
        Some(McpAttachmentRefusal::Denied)
    );
    fixture.change(|servers| servers.server_allowed.store(true, Ordering::SeqCst));

    let revoked = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("prepared");
    fixture.change(|servers| servers.server_allowed.store(false, Ordering::SeqCst));
    assert_eq!(
        revoked.authorize_apply(),
        Err(McpAttachmentRefusal::Denied),
        "a decision withdrawn while staged refuses at apply"
    );
    fixture.change(|servers| servers.server_allowed.store(true, Ordering::SeqCst));

    let reviewed = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("prepared");
    fixture.change(|servers| servers.review_required.store(true, Ordering::SeqCst));
    assert_eq!(
        reviewed.authorize_apply(),
        Err(McpAttachmentRefusal::ReviewRequired)
    );
    fixture.change(|servers| servers.review_required.store(false, Ordering::SeqCst));

    let retired = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("prepared");
    fixture
        .machine
        .retire_scoped_context(&fixture.registration, fixture.target.context_generation)
        .await
        .expect("retire the destination");
    assert_eq!(
        retired.authorize_apply(),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
            .await
            .err(),
        Some(McpAttachmentRefusal::NotCurrent),
        "no destination, nothing to attach to"
    );
}

/// The destination fact is joined with the actual binding: a control
/// naming another destination, or a target that is not the one the session
/// is currently bound to, never prepares.
#[tokio::test]
async fn the_destination_fact_is_joined_with_the_bound_target() {
    let fixture = Fixture::new().await;
    let mut facts = fixture.control().facts().clone();
    facts.destination.id = Arc::from("another-session");
    let foreign = McpAttachmentControlRequest::from_trusted_ingress(
        fixture.session.clone(),
        McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
        facts,
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &fixture.target, foreign)
            .await
            .err(),
        Some(McpAttachmentRefusal::Mismatch),
        "a claimed destination other than the bound one"
    );
    let mut stale = fixture.target.clone();
    stale.context_generation += 1;
    let control = McpAttachmentControlRequest::from_trusted_ingress(
        fixture.session.clone(),
        McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
        fixture.control().facts().clone(),
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &stale, control)
            .await
            .err(),
        Some(McpAttachmentRefusal::NotCurrent),
        "a target the session is not currently bound to"
    );
}

/// The attempt is reserved only at the actual entry: a refusal before entry
/// (a required review, then a denied decision) does not spend the prepared
/// control, which enters once the refusal clears, and only once.
#[tokio::test]
async fn a_refusal_before_entry_does_not_spend_the_attempt() {
    let fixture = Fixture::new().await;
    let attachment = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("prepared");
    fixture.change(|servers| servers.review_required.store(true, Ordering::SeqCst));
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::ReviewRequired)
    );
    fixture.change(|servers| {
        servers.server_allowed.store(false, Ordering::SeqCst);
        servers.review_required.store(false, Ordering::SeqCst);
    });
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::Denied)
    );
    fixture.change(|servers| servers.server_allowed.store(true, Ordering::SeqCst));
    attachment
        .authorize_apply()
        .expect("the same prepared control enters once the refusal clears");
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::NotCurrent),
        "and only once"
    );
}

/// A control for another session, or a session without its live executor
/// attachment, never prepares.
#[tokio::test]
async fn attachment_requires_the_exact_session_and_executor_attachment() {
    let fixture = Fixture::new().await;
    let foreign = McpAttachmentControlRequest::from_trusted_ingress(
        SessionId::new(),
        McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
        fixture.control().facts().clone(),
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &fixture.target, foreign)
            .await
            .err(),
        Some(McpAttachmentRefusal::Mismatch)
    );
    let detached = SessionId::new();
    fixture
        .machine
        .register_session(detached.clone())
        .await
        .expect("registration without an executor");
    let registration = fixture
        .machine
        .current_session_registration_witness(&detached)
        .await
        .expect("registration");
    let mut detached_target = fixture.target.clone();
    detached_target.logical_runtime =
        EvidenceId::new(MeerkatMachine::logical_runtime_id(&detached).to_string())
            .expect("runtime");
    let control = McpAttachmentControlRequest::from_trusted_ingress(
        detached,
        McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
        fixture.control().facts().clone(),
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&registration, &detached_target, control)
            .await
            .err(),
        Some(McpAttachmentRefusal::NotCurrent)
    );
}

struct NoOriginals;

impl crate::meerkat_machine::AuthenticatedOriginalSource for NoOriginals {
    fn authenticated_original(
        &self,
        _: &NativeIngressContext,
        _: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        Option<crate::meerkat_machine::AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(None)
    }
    fn retained_custody(
        &self,
        _: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        crate::meerkat_machine::RetainedCustody,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(crate::meerkat_machine::RetainedCustody::NotRetained)
    }
}

impl crate::meerkat_machine::ScopedMandateOwner for Mandate {
    fn authorize_invocation(
        &self,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), crate::input_authority::NativeAdmissionError> {
        Ok(())
    }
}

/// The scoped original composition's installed mandate owner hands an
/// attachment control to the host invocation policy (an attachment names no
/// contributor work), instead of failing every attachment as unavailable.
#[tokio::test]
async fn the_scoped_composition_resolves_the_attachment_mandate() {
    let fixture = Fixture::new().await;
    let (configuration, _, _, _) = mutable_controller_configuration(true);
    let composed = crate::meerkat_machine::ScopedGrantComposition {
        grants: Arc::clone(&configuration.grants),
        ingress_domain: fixture.servers.domain.clone(),
        originals: Arc::new(NoOriginals),
        invocation: crate::meerkat_machine::ScopedInvocation::from_policy(Arc::new(Mandate {
            inner: Arc::clone(&configuration.invocation_owner),
            requester: fixture.requester.clone(),
        })),
        operation_owner: Arc::clone(&configuration.operation_owner),
        retained_resume: None,
    }
    .into_configuration();
    let control = fixture.control();
    let binding =
        control.effect_binding(meerkat_core::mcp_attachment_control::McpAttachmentEffect::Apply);
    assert!(
        composed
            .invocation_owner
            .authorize_mcp_attachment(&control, &binding, 100)
            .is_ok(),
        "the host policy decides the attachment mandate"
    );
    let mut facts = control.facts().clone();
    facts.requester = PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount,
        "someone-else",
        meerkat_core::TrustDomainId::new("native-test").expect("domain"),
    )
    .expect("qualified");
    let other = McpAttachmentControlRequest::from_trusted_ingress(
        fixture.session.clone(),
        control.server().clone(),
        facts,
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert!(
        composed
            .invocation_owner
            .authorize_mcp_attachment(
                &other,
                &other.effect_binding(
                    meerkat_core::mcp_attachment_control::McpAttachmentEffect::Apply,
                ),
                100
            )
            .is_err(),
        "and refuses another requester"
    );
}

/// End to end on a machine built from the production scoped original
/// composition: the bound destination admits its attachment through the
/// composed mandate and operation owners, and another claimed destination
/// is refused.
#[tokio::test]
async fn the_scoped_composition_admits_the_bound_attachment_end_to_end() {
    let fixture = Fixture::scoped().await;
    let attachment = fixture
        .machine
        .prepare_mcp_attachment(&fixture.registration, &fixture.target, fixture.control())
        .await
        .expect("the composed owners allow the bound destination's attachment");
    attachment
        .authorize_apply()
        .expect("entered through the scoped composition");
    let mut facts = fixture.control().facts().clone();
    facts.destination.id = Arc::from("another-session");
    let foreign = McpAttachmentControlRequest::from_trusted_ingress(
        fixture.session.clone(),
        McpServerConfig::stdio("records", "records-mcp", Vec::new(), Default::default()),
        facts,
        Arc::new(AttachmentReceipt),
    )
    .expect("qualified control");
    assert_eq!(
        fixture
            .machine
            .prepare_mcp_attachment(&fixture.registration, &fixture.target, foreign)
            .await
            .err(),
        Some(McpAttachmentRefusal::Mismatch)
    );
}

/// On a per-agent host an attachment control is decided for the
/// registration's bound agent and the control's own server: unbound is
/// unavailable, a server not selected for the agent is unavailable, a
/// selected server attaches, and once its lineage is revoked the prepared
/// control is refused at entry. The other tests run the legacy host, where
/// attachments are unchanged.
#[tokio::test]
async fn a_per_agent_host_attaches_only_the_agents_selected_server() {
    let fixture = Fixture::per_agent().await;
    assert_eq!(
        fixture.prepare().await.err(),
        Some(McpAttachmentRefusal::Unavailable),
        "an unbound registration has no agent to attach for"
    );
    let granted = fixture.issue("records-grant", "records");
    fixture.select(&[("records", granted.clone())]).await;
    let attachment = fixture
        .prepare()
        .await
        .expect("the agent's selected server attaches");
    {
        let mut custody = fixture
            .machine
            .try_controller_grant_mutation()
            .expect("custody");
        fixture
            .grants
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &fixture.root,
                &granted,
                &mut custody,
            )
            .expect("revoke the selected lineage");
    }
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::Denied),
        "the server's hydration and publication are refused for the agent at entry"
    );

    let other = Fixture::per_agent().await;
    let elsewhere = other.issue("other-grant", "other");
    other.select(&[("other", elsewhere)]).await;
    assert_eq!(
        other.prepare().await.err(),
        Some(McpAttachmentRefusal::Unavailable),
        "a server not selected for the agent is unavailable on its own"
    );
}

/// The generic native/grant host gives MCP attachment controls the same
/// per-agent selection conjunct as its configured servers, from the same
/// agent grant owner and the registration's own binding: an otherwise
/// permitted control is refused locally without the agent's selection for
/// its server; for the selected agent it applies and (where the owner allows
/// the write) persists; and a retained prepared control is refused at entry
/// once the selected grant is revoked or the registration ends.
#[tokio::test]
async fn a_generic_per_agent_host_attaches_only_the_agents_selected_server() {
    let fixture = Fixture::generic_per_agent().await;
    assert_eq!(
        fixture.prepare().await.err(),
        Some(McpAttachmentRefusal::Unavailable),
        "an unbound registration has no agent to attach for"
    );
    let granted = fixture.issue("records-grant", "records");
    fixture.select(&[("records", granted.clone())]).await;
    let attachment = fixture
        .prepare()
        .await
        .expect("the agent's selected server attaches");
    // Independent controls for the selected agent enter: one live Apply,
    // and one configuration write once its owner allows writes.
    fixture
        .prepare()
        .await
        .expect("prepared for the selected agent")
        .authorize_apply()
        .expect("the selected agent's control applies");
    fixture.change(|servers| servers.persist_allowed.store(true, Ordering::SeqCst));
    fixture
        .prepare()
        .await
        .expect("prepared for the selected agent")
        .authorize_persist(McpScope::Project)
        .expect("the selected agent's control persists where its owner allows the write");
    {
        let mut custody = fixture
            .machine
            .try_controller_grant_mutation()
            .expect("custody");
        fixture
            .grants
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &fixture.root,
                &granted,
                &mut custody,
            )
            .expect("revoke the selected lineage");
    }
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::Denied),
        "the revoked selection is refused at entry"
    );

    let other = Fixture::generic_per_agent().await;
    let elsewhere = other.issue("other-grant", "other");
    other.select(&[("other", elsewhere)]).await;
    assert_eq!(
        other.prepare().await.err(),
        Some(McpAttachmentRefusal::Unavailable),
        "a server not selected for the agent is unavailable on its own"
    );

    let retired = Fixture::generic_per_agent().await;
    let granted = retired.issue("records-grant", "records");
    retired.select(&[("records", granted)]).await;
    let attachment = retired
        .prepare()
        .await
        .expect("the agent's selected server attaches");
    retired
        .machine
        .unregister_session(&retired.session)
        .await
        .expect("end the registration");
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::NotCurrent),
        "a control prepared for an ended registration is not current"
    );
}

/// The generic host without a per-agent provider keeps attachment controls
/// without the agent conjunct: an unbound registration still attaches.
#[tokio::test]
async fn a_generic_host_without_a_per_agent_provider_adds_no_agent_conjunct() {
    let fixture = Fixture::new().await;
    fixture
        .prepare()
        .await
        .expect("no per-agent provider: the control is decided without an agent");
}

/// The agent grant's clock, which once armed retires the session's scoped
/// destination from inside its `n`th read: native state changing while the
/// agent selection's owner code runs, with no grant publication.
#[derive(Default)]
struct RetiringClock {
    countdown: std::sync::atomic::AtomicUsize,
    target: std::sync::Mutex<
        Option<(
            std::sync::Weak<MeerkatMachine>,
            RuntimeSessionRegistrationWitness,
            u64,
        )>,
    >,
    retired: AtomicBool,
}

impl RetiringClock {
    fn arm(&self, fixture: &Fixture, nth: usize) {
        *self.target.lock().expect("clock") = Some((
            Arc::downgrade(&fixture.machine),
            fixture.registration.clone(),
            fixture.target.context_generation,
        ));
        self.countdown.store(nth, Ordering::SeqCst);
    }
}

impl meerkat_authorization::clock::LocalAuthorizationClock for RetiringClock {
    fn now(
        &self,
    ) -> Result<
        meerkat_authorization::work::LocalAuthorizationTime,
        meerkat_authorization::clock::LocalClockError,
    > {
        let due = self
            .countdown
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok_and(|left| left == 1);
        if due {
            let target = self.target.lock().expect("clock").take();
            if let Some((machine, registration, generation)) = target {
                let machine = machine.upgrade().expect("machine");
                // An idle native retirement completes without waiting.
                futures::FutureExt::now_or_never(
                    machine.retire_scoped_context(&registration, generation),
                )
                .expect("idle retirement completes")
                .expect("retire the destination");
                self.retired.store(true, Ordering::SeqCst);
            }
        }
        Clock.now()
    }
}

/// A destination retired while the entry's final agent selection runs is
/// caught by the native read that follows it: the effect is refused as not
/// current and never entered.
#[tokio::test]
async fn a_destination_retired_during_the_final_agent_selection_is_not_current() {
    let clock = Arc::new(RetiringClock::default());
    let fixture = Fixture::build(
        false,
        Some(Arc::clone(&clock)
            as Arc<
                dyn meerkat_authorization::clock::LocalAuthorizationClock,
            >),
        false,
    )
    .await;
    let granted = fixture.issue("records-grant", "records");
    fixture.select(&[("records", granted)]).await;
    let attachment = fixture.prepare().await.expect("prepared for the agent");
    // Entry reads the agent grant's clock twice (observe, confirm) in its
    // first selection; the third read is inside the final selection.
    clock.arm(&fixture, 3);
    assert_eq!(
        attachment.authorize_apply(),
        Err(McpAttachmentRefusal::NotCurrent),
        "the retired destination is caught after the final selection"
    );
    assert!(
        clock.retired.load(Ordering::SeqCst),
        "retired inside the final selection"
    );
    assert_eq!(
        fixture.prepare().await.err(),
        Some(McpAttachmentRefusal::NotCurrent),
        "no destination remains to attach to"
    );
}
