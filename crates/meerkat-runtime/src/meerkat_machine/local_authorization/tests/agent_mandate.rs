//! The per-agent configured-MCP conjunct over an actual governed machine:
//! the agent mandate bound to the registration, its exact current grant
//! lineage and the realm installation, decided before the session has any
//! input.
use super::*;
use crate::meerkat_machine::local_authorization::agent_grant::{
    AgentInstallationGrant, ConfiguredMcpConjuncts,
};
use crate::meerkat_machine::{AgentMandateError, SessionAgentMandate};
use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_authorization::mcp_installation::{
    MCP_INSTALLATION_FEATURE, mcp_connector_action, mcp_connector_domain,
};
use meerkat_authorization_contracts::constraints::{ExactRestriction, LifetimeRestriction};
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::mcp_config::McpConnectorBinding;
use meerkat_core::{McpConnectorApproval, McpServerConfig, PrincipalKind, TrustDomainId};

fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("agent-selection").expect("domain"),
    )
    .expect("principal")
}

/// The operator of `realm`: the authority of its server-registration domains.
fn operator_of(realm: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        "uid-1",
        TrustDomainId::new(format!("rkat-realm-{realm}")).expect("realm domain"),
    )
    .expect("operator")
}

/// The operator-owned domain of `server` in `realm`, as the stock owner names it.
fn server_domain(realm: &str, server: &str) -> ResourceDomain {
    ResourceDomain {
        authority: operator_of(realm),
        namespace: format!("mcp-server/{server}"),
    }
}

fn server(name: &str) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        "/nonexistent/mcp-server".to_string(),
        vec![],
        std::collections::HashMap::new(),
    )
}

/// A connector binding for `account`; every other field is fixed.
fn binding(account: &str) -> McpConnectorBinding {
    McpConnectorBinding {
        realm_id: "alpha".into(),
        slot_id: "records".into(),
        issuer: "https://issuer.example".into(),
        client: "client".into(),
        resource: "https://api.example".into(),
        strategy_id: "oidc".into(),
        account: account.into(),
        operations: std::collections::BTreeMap::new(),
    }
}

fn connector_server(name: &str, account: &str) -> McpServerConfig {
    McpServerConfig {
        connector: Some(binding(account)),
        account: None,
        ..server(name)
    }
}

/// The host composition's own installation decision: the installed names.
struct Installed(Vec<&'static str>);
impl ConfiguredMcpAttachmentAuthority for Installed {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        if self.0.contains(&server.name.as_str()) {
            Ok(())
        } else {
            Err(McpAttachmentRefusal::Denied)
        }
    }
}

/// The host's operation policy: refuses one server name.
struct PolicyRefuses(&'static str);
impl ConfiguredMcpAttachmentAuthority for PolicyRefuses {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        if server.name == self.0 {
            Err(McpAttachmentRefusal::Denied)
        } else {
            Ok(())
        }
    }
}

/// A host composition without its operation-policy owner yet.
struct WithoutPolicy {
    grant: AgentInstallationGrant,
}
impl NativeWorkAuthorizationHost for WithoutPolicy {
    fn configured_mcp_attachment_for_agent(
        &self,
        _session_id: &SessionId,
        agent: crate::meerkat_machine::SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        self.grant.for_agent(
            ConfiguredMcpConjuncts {
                installation: Arc::new(Installed(vec!["sentinel"])),
                policy: None,
            },
            agent,
        )
    }

    fn authenticate_association(
        &self,
        _: &LogicalRuntimeId,
        _: &Input,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), crate::input_authority::NativeAdmissionError> {
        Err(denied().into())
    }

    fn work_context(
        &self,
        _: &crate::input_authority::NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, crate::input_authority::NativeWorkContextError> {
        Err(crate::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// A governed host that decides configured servers per bound agent. It
/// admits no input; these controls never submit one.
struct PerAgentHost {
    grant: AgentInstallationGrant,
}
impl NativeWorkAuthorizationHost for PerAgentHost {
    fn configured_mcp_attachment_for_agent(
        &self,
        _session_id: &SessionId,
        agent: crate::meerkat_machine::SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        self.grant.for_agent(
            ConfiguredMcpConjuncts {
                installation: Arc::new(Installed(vec!["sentinel", "other", "unpermitted"])),
                policy: Some(Arc::new(PolicyRefuses("unpermitted"))),
            },
            agent,
        )
    }

    fn authenticate_association(
        &self,
        _: &LogicalRuntimeId,
        _: &Input,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), crate::input_authority::NativeAdmissionError> {
        Err(denied().into())
    }

    fn work_context(
        &self,
        _: &crate::input_authority::NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, crate::input_authority::NativeWorkContextError> {
        Err(crate::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// No unfinished controller work references these grants.
struct NoController;
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for NoController
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

struct Fixture {
    machine: MeerkatMachine,
    grants: Arc<LocalGrantAuthority>,
}

fn fixture() -> Fixture {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let grant =
        AgentInstallationGrant::new(Arc::clone(&grants), Arc::new(Clock), operator_of("alpha"));
    let machine = MeerkatMachine::ephemeral()
        .install_native_work_authorization(|_| Arc::new(PerAgentHost { grant }))
        .expect("exclusive setup");
    Fixture { machine, grants }
}

impl Fixture {
    fn issue(&self, id: &str, agent: &str, servers: &[&str], realm: &str) -> GrantLineageRef {
        self.issue_with(
            id,
            agent,
            servers,
            realm,
            LifetimeRestriction::unrestricted(),
        )
    }

    fn issue_with(
        &self,
        id: &str,
        agent: &str,
        servers: &[&str],
        realm: &str,
        lifetime: LifetimeRestriction,
    ) -> GrantLineageRef {
        self.grants
            .issue_root(
                &who("operator"),
                EvidenceId::new(id).expect("id"),
                who(agent),
                None,
                ExecutionRestrictions {
                    // Each server with its explicitly approved absence of a
                    // connector, as these servers have none.
                    actions: ExactRestriction::exact(servers.iter().flat_map(|server| {
                        [
                            ActionRef {
                                feature: MCP_INSTALLATION_FEATURE.to_owned(),
                                action: (*server).to_owned(),
                            },
                            mcp_connector_action(server, &McpConnectorApproval::NoConnector),
                        ]
                    })),
                    resource_domains: ExactRestriction::exact(
                        servers.iter().map(|server| server_domain(realm, server)),
                    ),
                    lifetime,
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("issue")
    }

    /// A grant for `server` in realm alpha recording exactly `approvals`
    /// (none at all: a grant issued before approvals were recorded).
    fn issue_approving(
        &self,
        id: &str,
        agent: &str,
        server: &str,
        approvals: &[McpConnectorApproval],
    ) -> GrantLineageRef {
        self.grants
            .issue_root(
                &who("operator"),
                EvidenceId::new(id).expect("id"),
                who(agent),
                None,
                ExecutionRestrictions {
                    actions: ExactRestriction::exact(
                        std::iter::once(ActionRef {
                            feature: MCP_INSTALLATION_FEATURE.to_owned(),
                            action: server.to_owned(),
                        })
                        .chain(
                            approvals
                                .iter()
                                .map(|approval| mcp_connector_action(server, approval)),
                        ),
                    ),
                    resource_domains: ExactRestriction::exact([server_domain("alpha", server)]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("issue")
    }

    /// A grant for every raw operation of `server` (unrestricted actions),
    /// approving exactly `approvals` through its resource domains.
    fn issue_every_operation(
        &self,
        id: &str,
        agent: &str,
        server: &str,
        approvals: &[McpConnectorApproval],
    ) -> GrantLineageRef {
        self.grants
            .issue_root(
                &who("operator"),
                EvidenceId::new(id).expect("id"),
                who(agent),
                None,
                ExecutionRestrictions {
                    resource_domains: ExactRestriction::exact(
                        std::iter::once(server_domain("alpha", server)).chain(
                            approvals.iter().map(|approval| {
                                mcp_connector_domain(&operator_of("alpha"), server, approval)
                            }),
                        ),
                    ),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("issue")
    }

    /// A registered session, its witness and its configured authority.
    async fn session(
        &self,
    ) -> (
        SessionId,
        crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        Arc<dyn ConfiguredMcpAttachmentAuthority>,
    ) {
        let session = SessionId::new();
        self.machine
            .register_session(session.clone())
            .await
            .expect("register");
        let witness = self
            .machine
            .current_session_registration_witness(&session)
            .await
            .expect("current registration");
        let bindings = self
            .machine
            .prepare_bindings(session.clone())
            .await
            .expect("bindings");
        let authority = bindings
            .configured_mcp_attachment()
            .cloned()
            .expect("governed authority");
        (session, witness, authority)
    }

    /// Select `lineage` for every server these tests name.
    async fn select(
        &self,
        witness: &crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        agent: &str,
        lineage: GrantLineageRef,
    ) -> Result<(), RuntimeDriverError> {
        let servers =
            ["sentinel", "other", "absent", "unpermitted"].map(|server| (server, lineage.clone()));
        self.select_per_server(witness, agent, &servers).await
    }

    /// Select each server's own lineage, and nothing for any other server.
    async fn select_per_server(
        &self,
        witness: &crate::meerkat_machine::RuntimeSessionRegistrationWitness,
        agent: &str,
        servers: &[(&str, GrantLineageRef)],
    ) -> Result<(), RuntimeDriverError> {
        let servers = servers
            .iter()
            .map(|(server, lineage)| ((*server).to_owned(), vec![lineage.clone()]))
            .collect();
        self.machine
            .bind_session_agent_mandate(
                witness,
                SessionAgentMandate::new(who(agent), servers, None, None).expect("mandate"),
            )
            .await
    }
}

/// The generic native/grant host with a per-agent configured-MCP provider:
/// the provider is asked with the registration's own binding slot and
/// composes the existing per-agent grant over it.
fn generic_fixture() -> Fixture {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let grant =
        AgentInstallationGrant::new(Arc::clone(&grants), Arc::new(Clock), operator_of("alpha"));
    let (mut configuration, _, _) = super::configuration();
    configuration.configured_mcp_attachment = Some(Arc::new(
        crate::meerkat_machine::AgentConfiguredMcpProvider::new(
            grant,
            Arc::new(Installed(vec!["sentinel", "other", "unpermitted"])),
            Some(Arc::new(PolicyRefuses("unpermitted"))),
        ),
    ));
    let machine = MeerkatMachine::ephemeral()
        .with_local_grant_authorization(configuration)
        .expect("exclusive setup");
    Fixture { machine, grants }
}

/// The generic host decides configured servers through the registration's
/// own binding: unavailable until the trusted host binds the agent, then the
/// bound agent's grant and the policy decide, and the registration's end is
/// observed by the authority assembled while it was current.
#[tokio::test]
async fn the_generic_host_decides_configured_servers_per_bound_agent() {
    let fx = generic_fixture();
    let a_grant = fx.issue(
        "a-sentinel",
        "agent-a",
        &["sentinel", "unpermitted"],
        "alpha",
    );
    let (session, witness, authority) = fx.session().await;
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "an unbound registration is unavailable, never permitted"
    );
    fx.select(&witness, "agent-a", a_grant)
        .await
        .expect("bind A after the authority was assembled");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Ok(()),
        "the same authority reads the binding made after it was assembled"
    );
    assert_eq!(
        authority.authorize_configured(&server("other")),
        Err(McpAttachmentRefusal::Denied),
        "installed but not granted to A"
    );
    assert_eq!(
        authority.authorize_configured(&server("unpermitted")),
        Err(McpAttachmentRefusal::Denied),
        "the policy conjunct still applies"
    );

    fx.machine
        .unregister_session(&session)
        .await
        .expect("unregister");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::NotCurrent),
        "the authority of an ended registration is not current"
    );
    fx.machine
        .register_session(session.clone())
        .await
        .expect("register again");
    let replacement = fx
        .machine
        .prepare_bindings(session)
        .await
        .expect("bindings")
        .configured_mcp_attachment()
        .cloned()
        .expect("authority");
    assert_eq!(
        replacement.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "a replacement registration starts unbound"
    );
}

/// Without a configured provider the generic host refuses every configured
/// server, bound agent or not.
#[tokio::test]
async fn the_generic_host_without_a_provider_refuses_configured_servers() {
    let (configuration, _, _) = super::configuration();
    let machine = MeerkatMachine::ephemeral()
        .with_local_grant_authorization(configuration)
        .expect("exclusive setup");
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("register");
    let authority = machine
        .prepare_bindings(session)
        .await
        .expect("bindings")
        .configured_mcp_attachment()
        .cloned()
        .expect("governed authority");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable)
    );
}

#[tokio::test]
async fn the_installation_is_granted_per_selected_agent_before_any_input() {
    let fx = fixture();
    // A's grant also names "absent", which the host has not installed, so
    // the refusal for it below can only come from the installation check.
    let a_grant = fx.issue("a-sentinel", "agent-a", &["sentinel", "absent"], "alpha");
    let b_grant = fx.issue("b-other", "agent-b", &["other"], "alpha");

    // Agent A: the same installation is admitted, before any input exists.
    let (_, a_witness, a_authority) = fx.session().await;
    assert_eq!(
        a_authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "an unbound registration is unavailable, never permitted"
    );
    fx.select(&a_witness, "agent-a", a_grant.clone())
        .await
        .expect("select A");
    assert_eq!(
        a_authority.authorize_configured(&server("sentinel")),
        Ok(())
    );
    assert_eq!(
        a_authority.authorize_configured(&server("other")),
        Err(McpAttachmentRefusal::Denied),
        "installed but not granted to A"
    );
    assert_eq!(
        a_authority.authorize_configured(&server("absent")),
        Err(McpAttachmentRefusal::Denied),
        "granted and allowed by policy, but not installed: the installation check refuses"
    );

    // Granted to the agent, but refused by the host's operation policy.
    let unpermitted = fx.issue("a-unpermitted", "agent-a", &["unpermitted"], "alpha");
    let (_, policy_witness, policy_authority) = fx.session().await;
    fx.select(&policy_witness, "agent-a", unpermitted)
        .await
        .expect("bind");
    assert_eq!(
        policy_authority.authorize_configured(&server("unpermitted")),
        Err(McpAttachmentRefusal::Denied),
        "the policy conjunct still applies to a granted installation"
    );

    // Agent B on the same installation: never admitted.
    let (_, b_witness, b_authority) = fx.session().await;
    fx.select(&b_witness, "agent-b", b_grant)
        .await
        .expect("select B");
    assert_eq!(
        b_authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied)
    );
    assert_eq!(b_authority.authorize_configured(&server("other")), Ok(()));

    // B cannot borrow A's lineage: it does not resolve for B.
    let (_, borrowed_witness, borrowed) = fx.session().await;
    fx.select(&borrowed_witness, "agent-b", a_grant)
        .await
        .expect("select B with A's lineage");
    assert_eq!(
        borrowed.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied)
    );
}

/// A grant covers a configured server only with the exact connector binding
/// it approved: a same-name server rebound to another account, or given a
/// connector after a no-connector approval, is refused locally before any
/// spawn; a grant that recorded no approval is unavailable rather than read
/// as either; the same binding (as after a token refresh) and an explicit
/// regrant for the new account are admitted.
#[tokio::test]
async fn a_grant_covers_only_the_connector_binding_it_approved() {
    let fx = fixture();
    let account_a = McpConnectorApproval::of(Some(&binding("account-a")));
    let account_b = McpConnectorApproval::of(Some(&binding("account-b")));
    assert_ne!(account_a, account_b);

    let grant_a = fx.issue_approving("a-sentinel-a", "agent-a", "sentinel", &[account_a]);
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", grant_a)
        .await
        .expect("select");
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-a")),
        Ok(())
    );
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-a")),
        Ok(()),
        "the same binding again (a refreshed credential changes none of it)"
    );
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-b")),
        Err(McpAttachmentRefusal::Denied),
        "a same-name server rebound to another account is not approved"
    );
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied),
        "an approved connector is not an approval of no connector"
    );

    let none = fx.issue_approving(
        "a-sentinel-none",
        "agent-a",
        "sentinel",
        &[McpConnectorApproval::NoConnector],
    );
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", none).await.expect("select");
    assert_eq!(authority.authorize_configured(&server("sentinel")), Ok(()));
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-a")),
        Err(McpAttachmentRefusal::Denied),
        "a connector added to a no-connector server is not approved"
    );

    let legacy = fx.issue_approving("a-sentinel-legacy", "agent-a", "sentinel", &[]);
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", legacy)
        .await
        .expect("select");
    for configured in [
        server("sentinel"),
        connector_server("sentinel", "account-a"),
    ] {
        assert_eq!(
            authority.authorize_configured(&configured),
            Err(McpAttachmentRefusal::Unavailable),
            "a grant without a recorded approval needs an explicit one"
        );
    }

    let grant_b = fx.issue_approving("a-sentinel-b", "agent-a", "sentinel", &[account_b]);
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", grant_b)
        .await
        .expect("select");
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-b")),
        Ok(()),
        "an explicit regrant approves the new account"
    );
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-a")),
        Err(McpAttachmentRefusal::Denied)
    );
}

/// A grant for every raw operation (unrestricted actions) approves no
/// connector through that wildcard: without a recorded approval every
/// same-name descriptor is unavailable, and with one only that binding is
/// admitted.
#[tokio::test]
async fn an_every_operation_grant_approves_only_the_connector_its_domains_name() {
    let fx = fixture();
    let account_a = McpConnectorApproval::of(Some(&binding("account-a")));

    let unapproved = fx.issue_every_operation("a-all-none", "agent-a", "sentinel", &[]);
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", unapproved)
        .await
        .expect("select");
    for configured in [
        connector_server("sentinel", "account-a"),
        connector_server("sentinel", "account-b"),
        server("sentinel"),
    ] {
        assert_eq!(
            authority.authorize_configured(&configured),
            Err(McpAttachmentRefusal::Unavailable),
            "the operation wildcard approves no connector"
        );
    }

    let approved = fx.issue_every_operation("a-all-a", "agent-a", "sentinel", &[account_a]);
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", approved)
        .await
        .expect("select");
    assert_eq!(
        authority.authorize_configured(&connector_server("sentinel", "account-a")),
        Ok(())
    );
    for configured in [
        connector_server("sentinel", "account-b"),
        server("sentinel"),
    ] {
        assert_eq!(
            authority.authorize_configured(&configured),
            Err(McpAttachmentRefusal::Denied),
            "a same-name descriptor with another binding"
        );
    }
}

#[tokio::test]
async fn revoked_expired_and_other_realm_grants_refuse_at_each_decision() {
    let fx = fixture();
    let grant = fx.issue("a-sentinel", "agent-a", &["sentinel"], "alpha");
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", grant.clone())
        .await
        .expect("select");
    assert_eq!(authority.authorize_configured(&server("sentinel")), Ok(()));
    fx.grants
        .revoke(&who("operator"), &grant, &mut NoController)
        .expect("revoke");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied),
        "the authority minted before revocation rechecks the current grant"
    );

    // The test clock reads 100: a window ending at 50 has expired.
    let expired = fx.issue_with(
        "a-expired",
        "agent-a",
        &["sentinel"],
        "alpha",
        LifetimeRestriction::window(1, 50),
    );
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", expired)
        .await
        .expect("select");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied)
    );

    let beta = fx.issue("a-beta", "agent-a", &["sentinel"], "beta");
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", beta).await.expect("select");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied),
        "a grant for another realm's installation of the same name"
    );
}

#[tokio::test]
async fn a_registration_binds_once_and_a_replacement_starts_unbound() {
    let fx = fixture();
    let a_grant = fx.issue("a-sentinel", "agent-a", &["sentinel"], "alpha");
    let b_grant = fx.issue("b-sentinel", "agent-b", &["sentinel"], "alpha");
    let (session, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", a_grant.clone())
        .await
        .expect("select A");
    fx.select(&witness, "agent-a", a_grant.clone())
        .await
        .expect("the same selection again is a no-op");
    assert!(
        fx.select(&witness, "agent-b", b_grant.clone())
            .await
            .is_err(),
        "a registration's agent is never relabeled"
    );
    assert_eq!(authority.authorize_configured(&server("sentinel")), Ok(()));

    fx.machine
        .unregister_session(&session)
        .await
        .expect("unregister");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::NotCurrent),
        "the authority minted while the registration was current is not current after it ended"
    );
    assert!(
        fx.select(&witness, "agent-a", a_grant.clone())
            .await
            .is_err(),
        "an old registration's witness cannot bind"
    );
    fx.machine
        .register_session(session.clone())
        .await
        .expect("register again");
    let bindings = fx
        .machine
        .prepare_bindings(session.clone())
        .await
        .expect("bindings");
    let replacement = bindings
        .configured_mcp_attachment()
        .cloned()
        .expect("authority");
    assert_eq!(
        replacement.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "a replacement registration never inherits the old mandate"
    );
    let current = fx
        .machine
        .current_session_registration_witness(&session)
        .await
        .expect("current");
    fx.select(&current, "agent-b", b_grant)
        .await
        .expect("the replacement selects its own agent");
    assert_eq!(
        replacement.authorize_configured(&server("sentinel")),
        Ok(())
    );
}

#[test]
fn a_mandate_needs_a_selection() {
    assert_eq!(
        SessionAgentMandate::new(who("agent-a"), Default::default(), None, None).map(|_| ()),
        Err(AgentMandateError::NoSelection)
    );
}

#[tokio::test]
async fn each_server_resolves_only_the_lineage_selected_for_it() {
    let fx = fixture();
    // Both grants cover both servers, so any refusal below comes from which
    // lineage was selected for which server, never from grant coverage.
    let first = fx.issue("a-first", "agent-a", &["sentinel", "other"], "alpha");
    let second = fx.issue("a-second", "agent-a", &["sentinel", "other"], "alpha");
    let (_, witness, authority) = fx.session().await;
    fx.select_per_server(
        &witness,
        "agent-a",
        &[("sentinel", first.clone()), ("other", second.clone())],
    )
    .await
    .expect("select per server");
    assert_eq!(authority.authorize_configured(&server("sentinel")), Ok(()));
    assert_eq!(authority.authorize_configured(&server("other")), Ok(()));
    assert_eq!(
        authority.authorize_configured(&server("unpermitted")),
        Err(McpAttachmentRefusal::Unavailable),
        "a server with no selected lineage is unavailable on its own"
    );

    fx.grants
        .revoke(&who("operator"), &first, &mut NoController)
        .expect("revoke the first lineage");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied),
        "the revoked lineage refuses the server it was selected for"
    );
    assert_eq!(
        authority.authorize_configured(&server("other")),
        Ok(()),
        "another server's selected lineage is unaffected, and never lends its grant"
    );
}

#[tokio::test]
async fn a_missing_policy_owner_fails_closed() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let grant =
        AgentInstallationGrant::new(Arc::clone(&grants), Arc::new(Clock), operator_of("alpha"));
    let machine = MeerkatMachine::ephemeral()
        .install_native_work_authorization(|_| Arc::new(WithoutPolicy { grant }))
        .expect("exclusive setup");
    let fx = Fixture { machine, grants };
    let lineage = fx.issue("a-sentinel", "agent-a", &["sentinel"], "alpha");
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", lineage).await.expect("bind");
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "installed and granted, but no policy owner: never permitted"
    );
}

/// A policy owner that changes grant state during its own decision and then
/// permits: it revokes the bound grant, or issues an unrelated grant (which
/// also publishes).
struct MutatingPolicy {
    grants: Arc<LocalGrantAuthority>,
    revoke: std::sync::Mutex<Option<GrantLineageRef>>,
}
impl ConfiguredMcpAttachmentAuthority for MutatingPolicy {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        match self.revoke.lock().expect("slot").take() {
            Some(grant) => self
                .grants
                .revoke(&who("operator"), &grant, &mut NoController)
                .expect("revoke inside policy"),
            None => {
                self.grants
                    .issue_root(
                        &who("operator"),
                        EvidenceId::new(format!("unrelated-{}", uuid::Uuid::new_v4())).expect("id"),
                        who("someone-else"),
                        None,
                        ExecutionRestrictions::unrestricted(),
                    )
                    .expect("unrelated issuance inside policy");
            }
        }
        Ok(())
    }
}

struct MutatingPolicyHost {
    grant: AgentInstallationGrant,
    policy: Arc<MutatingPolicy>,
}
impl NativeWorkAuthorizationHost for MutatingPolicyHost {
    fn configured_mcp_attachment_for_agent(
        &self,
        _session_id: &SessionId,
        agent: crate::meerkat_machine::SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        self.grant.for_agent(
            ConfiguredMcpConjuncts {
                installation: Arc::new(Installed(vec!["sentinel"])),
                policy: Some(Arc::clone(&self.policy) as Arc<dyn ConfiguredMcpAttachmentAuthority>),
            },
            agent,
        )
    }

    fn authenticate_association(
        &self,
        _: &LogicalRuntimeId,
        _: &Input,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), crate::input_authority::NativeAdmissionError> {
        Err(denied().into())
    }

    fn work_context(
        &self,
        _: &crate::input_authority::NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, crate::input_authority::NativeWorkContextError> {
        Err(crate::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

#[tokio::test]
async fn a_grant_change_during_the_policy_decision_refuses_the_composite() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let policy = Arc::new(MutatingPolicy {
        grants: Arc::clone(&grants),
        revoke: std::sync::Mutex::new(None),
    });
    let grant =
        AgentInstallationGrant::new(Arc::clone(&grants), Arc::new(Clock), operator_of("alpha"));
    let host_policy = Arc::clone(&policy);
    let machine = MeerkatMachine::ephemeral()
        .install_native_work_authorization(|_| {
            Arc::new(MutatingPolicyHost {
                grant,
                policy: host_policy,
            })
        })
        .expect("exclusive setup");
    let fx = Fixture { machine, grants };
    let lineage = fx.issue("a-sentinel", "agent-a", &["sentinel"], "alpha");
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", lineage.clone())
        .await
        .expect("bind");

    // The policy publishes an unrelated grant and permits: the observation
    // the grant conjunct made is no longer current.
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::NotCurrent)
    );

    // The policy revokes the bound grant and permits: refused, and a fresh
    // decision then sees the revocation.
    *policy.revoke.lock().expect("slot") = Some(lineage);
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Denied)
    );
}

/// The host's installation decision, which here interferes with the grant
/// owner before the per-agent grant is resolved (an owner change begun and
/// held on another thread, or a poisoned grant publication) and then
/// permits.
struct InterferingInstallation {
    pending: std::sync::Mutex<Option<GrantInterference>>,
}

enum GrantInterference {
    HoldOwnerChange {
        begin: std::sync::mpsc::SyncSender<()>,
        held: std::sync::mpsc::Receiver<()>,
    },
    Poison(LocalAuthorizationPublication),
}

impl ConfiguredMcpAttachmentAuthority for InterferingInstallation {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        match self.pending.lock().expect("slot").take() {
            Some(GrantInterference::HoldOwnerChange { begin, held }) => {
                begin.send(()).expect("ask for an owner change");
                held.recv_timeout(std::time::Duration::from_secs(5))
                    .expect("the owner change is held");
            }
            Some(GrantInterference::Poison(publication)) => publication.poison(),
            None => {}
        }
        Ok(())
    }
}

struct InterferingInstallationHost {
    grant: AgentInstallationGrant,
    installation: Arc<InterferingInstallation>,
}
impl NativeWorkAuthorizationHost for InterferingInstallationHost {
    fn configured_mcp_attachment_for_agent(
        &self,
        _session_id: &SessionId,
        agent: crate::meerkat_machine::SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        self.grant.for_agent(
            ConfiguredMcpConjuncts {
                installation: Arc::clone(&self.installation)
                    as Arc<dyn ConfiguredMcpAttachmentAuthority>,
                policy: Some(Arc::new(PolicyRefuses("unpermitted"))),
            },
            agent,
        )
    }

    fn authenticate_association(
        &self,
        _: &LogicalRuntimeId,
        _: &Input,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), crate::input_authority::NativeAdmissionError> {
        Err(denied().into())
    }

    fn work_context(
        &self,
        _: &crate::input_authority::NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, crate::input_authority::NativeWorkContextError> {
        Err(crate::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// The grant owner's clock, failing on demand.
struct SwitchClock(std::sync::atomic::AtomicBool);
impl LocalAuthorizationClock for SwitchClock {
    fn now(&self) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(meerkat_authorization::clock::LocalClockError::Unavailable);
        }
        Clock.now()
    }
}

/// An owner change that publishes while the per-agent grant's lineage is
/// read is a change during the decision: not current, as for one published
/// after the grant was observed, never an unavailable grant owner. A fresh
/// decision once the change completes permits. A failing grant owner clock
/// or a poison at the same point stays unavailable.
#[tokio::test]
async fn a_grant_change_while_the_agent_grant_is_read_is_not_current() {
    let publication = LocalAuthorizationPublication::new();
    let clock = Arc::new(SwitchClock(std::sync::atomic::AtomicBool::new(false)));
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            publication.clone(),
            Arc::clone(&clock) as Arc<dyn LocalAuthorizationClock>,
        )
        .expect("grant owner"),
    );
    let installation = Arc::new(InterferingInstallation {
        pending: std::sync::Mutex::new(None),
    });
    let grant =
        AgentInstallationGrant::new(Arc::clone(&grants), Arc::new(Clock), operator_of("alpha"));
    let host_installation = Arc::clone(&installation);
    let machine = MeerkatMachine::ephemeral()
        .install_native_work_authorization(|_| {
            Arc::new(InterferingInstallationHost {
                grant,
                installation: host_installation,
            })
        })
        .expect("exclusive setup");
    let fx = Fixture { machine, grants };
    let lineage = fx.issue("a-sentinel", "agent-a", &["sentinel"], "alpha");
    let (_, witness, authority) = fx.session().await;
    fx.select(&witness, "agent-a", lineage).await.expect("bind");

    let (begin_tx, begin_rx) = std::sync::mpsc::sync_channel(1);
    let (held_tx, held_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let changer_publication = publication.clone();
    let changer = std::thread::spawn(move || {
        begin_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("asked to change an owner");
        let change = changer_publication
            .begin_owner_change()
            .expect("an owner change in progress");
        held_tx.send(()).expect("the change is held");
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("released by the test");
        drop(change);
    });
    *installation.pending.lock().expect("slot") = Some(GrantInterference::HoldOwnerChange {
        begin: begin_tx,
        held: held_rx,
    });
    let during = authority.authorize_configured(&server("sentinel"));
    release_tx.send(()).expect("release the owner change");
    changer.join().expect("changer thread");
    assert_eq!(
        during,
        Err(McpAttachmentRefusal::NotCurrent),
        "a change published while the grant is read is not current, never unavailable"
    );
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Ok(()),
        "a fresh decision once the change completes"
    );

    clock.0.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "a failing grant owner clock stays unavailable"
    );
    clock.0.store(false, std::sync::atomic::Ordering::SeqCst);

    *installation.pending.lock().expect("slot") = Some(GrantInterference::Poison(publication));
    assert_eq!(
        authority.authorize_configured(&server("sentinel")),
        Err(McpAttachmentRefusal::Unavailable),
        "a poison stays unavailable"
    );
}
