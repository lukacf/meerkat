//! The per-agent conjunct of a governed host's configured-MCP decision.
//!
//! A realm installation does not grant every agent: the agent mandate the
//! trusted host bound to the session registration must currently hold a
//! grant that covers the installation. This resolves the exact lineage the
//! bound mandate selected for that server, for that executor and represented
//! subject, through the existing grant owner, under its native publication,
//! at every decision. It never selects
//! an agent, issues a grant or reads an input's association, and it is one
//! conjunct only: [`AgentInstallationGrant::for_agent`] conjoins it after the
//! host's installation decision and before the host's operation policy for
//! the attachment.

use std::sync::Arc;

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grants::{GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority};
use meerkat_authorization::mcp_installation::{
    McpConnectorBound, McpInstallationRef, check_resolved_installation, lineage_connector_bound,
};
use meerkat_authorization_contracts::constraints::{
    BoundMatch, ResourceDomain, RestrictionFailure,
};
use meerkat_core::McpServerConfig;
use meerkat_core::auth::PrincipalRef;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};

use crate::meerkat_machine::{AgentBindingUnavailable, SessionAgentBinding};

/// Every present conjunct of one registration's configured-MCP decision.
pub struct ConfiguredMcpConjuncts {
    /// The host's own installation decision (for example the realm's
    /// persisted S7 installation at its current generation).
    pub installation: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    /// The host's native operation policy for the attachment. Until the
    /// host composition supplies its owner (`None`), every decision is
    /// refused as unavailable: the attachment is never permitted without it.
    pub policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
}

/// The per-agent configured-MCP provider of a generic governed host: one
/// [`AgentInstallationGrant`] decides each configured server (after the
/// host's installation decision and before its operation policy) and the
/// agent selection of each MCP attachment control, both for the
/// registration's own agent binding at every decision.
#[derive(Clone)]
pub struct AgentConfiguredMcpProvider {
    grant: AgentInstallationGrant,
    installation: Arc<dyn ConfiguredMcpAttachmentAuthority>,
    policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
}

impl AgentConfiguredMcpProvider {
    /// `installation` and `policy` are the conjuncts of
    /// [`ConfiguredMcpConjuncts`]; a missing policy refuses every configured
    /// server as unavailable.
    pub fn new(
        grant: AgentInstallationGrant,
        installation: Arc<dyn ConfiguredMcpAttachmentAuthority>,
        policy: Option<Arc<dyn ConfiguredMcpAttachmentAuthority>>,
    ) -> Self {
        Self {
            grant,
            installation,
            policy,
        }
    }
}

impl super::ConfiguredMcpAttachmentProvider for AgentConfiguredMcpProvider {
    fn configured(
        &self,
        _session_id: &meerkat_core::SessionId,
        agent: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        Some(self.grant.for_agent(
            ConfiguredMcpConjuncts {
                installation: Arc::clone(&self.installation),
                policy: self.policy.clone(),
            },
            agent,
        ))
    }

    fn agent_selection(
        &self,
        agent: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        Some(self.grant.agent_selection(agent))
    }
}

/// The grant owner, its clock and the realm's installation domain.
#[derive(Clone)]
pub struct AgentInstallationGrant {
    grants: Arc<LocalGrantAuthority>,
    clock: Arc<dyn LocalAuthorizationClock>,
    /// The realm's operator: the authority of every MCP server-registration
    /// domain (`mcp-server/<server>`), as the stock operation owner names it.
    operator: PrincipalRef,
}

impl std::fmt::Debug for AgentInstallationGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInstallationGrant")
            .finish_non_exhaustive()
    }
}

impl AgentInstallationGrant {
    /// `operator` is the realm's operator principal. Each installed server
    /// is the operator-owned domain `mcp-server/<server name>`, the same
    /// domain the stock operation owner uses for that server's raw
    /// operations; the operator's realm trust domain qualifies the realm.
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        clock: Arc<dyn LocalAuthorizationClock>,
        operator: PrincipalRef,
    ) -> Self {
        Self {
            grants,
            clock,
            operator,
        }
    }

    /// The server-registration domain of `server`.
    fn server_domain(&self, server: &McpServerConfig) -> ResourceDomain {
        ResourceDomain {
            authority: self.operator.clone(),
            namespace: format!("mcp-server/{}", server.name),
        }
    }

    /// Whether the registration's bound agent currently holds a grant
    /// covering `server`, through the lineage its mandate selected for that
    /// server. An unbound registration, or one whose mandate selected no
    /// lineage for `server`, is unavailable; a lineage
    /// that does not resolve for the bound executor and subject, or that is
    /// revoked, expired or does not name the installation, is denied; a
    /// publication change during the decision, including one published
    /// while the lineage is read, is not current.
    pub fn authorize(
        &self,
        agent: &SessionAgentBinding,
        server: &McpServerConfig,
    ) -> Result<(), McpAttachmentRefusal> {
        let observed = self.observe(agent, server)?;
        self.confirm(&observed, agent)
    }

    /// Resolve and check the bound grant once, keeping that exact
    /// observation so later conjuncts can be bracketed by it.
    fn observe(
        &self,
        agent: &SessionAgentBinding,
        server: &McpServerConfig,
    ) -> Result<ObservedAgentGrant, McpAttachmentRefusal> {
        let mandate = agent.current().map_err(binding_refusal)?;
        // The approval of the descriptor actually being attached; the grant
        // must have recorded exactly this one.
        let connector = server.connector_approval();
        let installation =
            McpInstallationRef::new(self.server_domain(server), &server.name, &connector)
                .map_err(|_| McpAttachmentRefusal::Mismatch)?;
        // Only the lineage selected for this server is resolved: nothing is
        // borrowed from another server's selection, and a server without one
        // is unavailable on its own.
        let lineage = mandate
            .server_lineage(&server.name)
            .ok_or(McpAttachmentRefusal::Unavailable)?;
        let resolved = self
            .grants
            .resolve_lineage_observed(lineage, mandate.executor(), mandate.represented_subject())
            .map_err(|refusal| match refusal {
                // A change published while the lineage is read is a change
                // during this decision, as one published before `confirm`
                // is: not current, never an unavailable owner.
                LineageResolutionRefusal::Changed => McpAttachmentRefusal::NotCurrent,
                LineageResolutionRefusal::Refused(GrantRefusal::Denied) => {
                    McpAttachmentRefusal::Denied
                }
                LineageResolutionRefusal::Refused(
                    GrantRefusal::ControllerInUse
                    | GrantRefusal::Duplicate
                    | GrantRefusal::Unavailable,
                ) => McpAttachmentRefusal::Unavailable,
            })?;
        let now_ms = self
            .clock
            .now()
            .map_err(|_| McpAttachmentRefusal::Unavailable)?
            .unix_ms;
        check_resolved_installation(&resolved, &installation, now_ms).map_err(|failure| {
            match failure {
                // A grant that covers this installation (its action and
                // domain) but recorded no connector approval for it is
                // missing evidence, not a verdict: it needs an explicit
                // approval and never adopts the current one.
                RestrictionFailure::OutsideBounds { .. }
                    if resolved.restrictions().actions.check(installation.action())
                        == BoundMatch::Matches
                        && resolved
                            .restrictions()
                            .resource_domains
                            .check(installation.domain())
                            == BoundMatch::Matches
                        && lineage_connector_bound(
                            resolved.lineage_restrictions(),
                            resolved.restrictions(),
                            installation.connector(),
                            installation.domain(),
                        ) == McpConnectorBound::Unrecorded =>
                {
                    McpAttachmentRefusal::Unavailable
                }
                RestrictionFailure::OutsideBounds { .. } => McpAttachmentRefusal::Denied,
                RestrictionFailure::Unresolved { .. } => McpAttachmentRefusal::Unavailable,
            }
        })?;
        Ok(ObservedAgentGrant {
            mandate: mandate.clone(),
            resolved,
        })
    }

    /// The same observation still holds: the binding is the observed mandate,
    /// the lineage has not expired and the grant owner's publication has not
    /// changed since it was resolved (a revocation or any other grant change
    /// publishes). Never re-resolves: a fresh stamp would bless old facts.
    fn confirm(
        &self,
        observed: &ObservedAgentGrant,
        agent: &SessionAgentBinding,
    ) -> Result<(), McpAttachmentRefusal> {
        if agent.current().map_err(binding_refusal)? != &observed.mandate {
            return Err(McpAttachmentRefusal::NotCurrent);
        }
        let now_ms = self
            .clock
            .now()
            .map_err(|_| McpAttachmentRefusal::Unavailable)?
            .unix_ms;
        if now_ms >= observed.resolved.expires_at_ms() {
            return Err(McpAttachmentRefusal::Denied);
        }
        observed
            .resolved
            .check_current()
            .map_err(|_| McpAttachmentRefusal::NotCurrent)
    }

    /// The configured-MCP authority of one registration, for
    /// `NativeWorkAuthorizationHost::configured_mcp_attachment_for_agent`:
    /// the host's installation decision, then this per-agent grant, then the
    /// host's operation policy, all at every decision. A missing policy owner
    /// fails closed as unavailable.
    pub fn for_agent(
        &self,
        conjuncts: ConfiguredMcpConjuncts,
        agent: SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        Arc::new(AgentGrantedConfiguredMcp {
            conjuncts,
            grant: self.clone(),
            agent,
        })
    }
}

impl AgentInstallationGrant {
    /// This per-agent grant alone, as the selection conjunct of an MCP
    /// attachment control for `agent`'s registration
    /// (`NativeWorkAuthorizationHost::mcp_attachment_agent_selection`): the
    /// control's server must have a lineage selected for the bound agent that
    /// currently resolves and names the installation.
    pub fn agent_selection(
        &self,
        agent: SessionAgentBinding,
    ) -> Arc<dyn ConfiguredMcpAttachmentAuthority> {
        Arc::new(AgentSelectedServer {
            grant: self.clone(),
            agent,
        })
    }
}

struct AgentSelectedServer {
    grant: AgentInstallationGrant,
    agent: SessionAgentBinding,
}

impl ConfiguredMcpAttachmentAuthority for AgentSelectedServer {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        self.grant.authorize(&self.agent, server)
    }
}

/// A never-bound registration has no agent to decide for; a retired one
/// ended, so anything decided for it is no longer current. By type only.
fn binding_refusal(reason: AgentBindingUnavailable) -> McpAttachmentRefusal {
    match reason {
        AgentBindingUnavailable::Unbound => McpAttachmentRefusal::Unavailable,
        AgentBindingUnavailable::Retired => McpAttachmentRefusal::NotCurrent,
    }
}

/// One resolved grant observation, held across the composite decision.
struct ObservedAgentGrant {
    mandate: crate::meerkat_machine::SessionAgentMandate,
    resolved: meerkat_authorization::grants::ResolvedGrant,
}

struct AgentGrantedConfiguredMcp {
    conjuncts: ConfiguredMcpConjuncts,
    grant: AgentInstallationGrant,
    agent: SessionAgentBinding,
}

impl ConfiguredMcpAttachmentAuthority for AgentGrantedConfiguredMcp {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        self.conjuncts.installation.authorize_configured(server)?;
        let observed = self.grant.observe(&self.agent, server)?;
        self.grant.confirm(&observed, &self.agent)?;
        match &self.conjuncts.policy {
            Some(policy) => policy.authorize_configured(server)?,
            None => return Err(McpAttachmentRefusal::Unavailable),
        }
        // The policy owner runs after the grant was observed and may change
        // grant state; the same observation must still hold afterwards.
        self.grant.confirm(&observed, &self.agent)
    }
}
