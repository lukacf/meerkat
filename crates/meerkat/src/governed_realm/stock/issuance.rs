//! The operator's explicit issuance of a stock realm's agent grants.
//!
//! [`grant_stock_agent`] (and `rkat realm grant`, for every target but a
//! tool bundle operation or a checked route) issues one grant per target to
//! a named agent of a governed realm, through the realm's restored grant
//! owner, as the authenticated local operator, under the realm's exclusive
//! governed execution custody: one installed MCP server, one trusted host's
//! checked route to an installed MCP server, one operation of a tool
//! bundle a trusted host registers, the tool catalog, or the agent's
//! controller. Each grant has its own exact scope and lifetime, and
//! the realm's grant selection (trusted host configuration) records only its
//! reference, per target. The grant owner resolves every reference at use.
//! Nothing is issued when a realm is governed or opened.
//!
//! The stock vocabulary: an MCP server grant covers the operator-owned
//! `mcp-server/<server>` domain with `mcp/<raw operation>` actions (or every
//! action) and the `mcp_installation/<server>` entitlement; a checked route
//! grant has the same exact shape for the operations a trusted host's checked
//! route to that server offers, and is selected for exactly that route (the
//! server's own grant never covers the route, nor the route's grant the
//! server's ordinary operations); a tool bundle
//! operation grant covers exactly `tool_bundle/<operation>` on the
//! operator-owned `tool-bundle/<bundle>` domain, where `<bundle>` is the
//! stable name the trusted host registers the bundle under (it is never
//! pinned to one registration instance, and a catalog grant never covers
//! it); the catalog grant covers `stock/catalog-read` on the operator's
//! `tool-catalog`; the
//! controller grant covers `stock/infer` on exactly the controller routes it
//! names, each by the key of that route's actual lowering
//! ([`ControllerRouteKey`]): provider, model, account binding, endpoint and
//! wire model. A route the grant does not name is refused, including after
//! the route itself changes; the operator then issues a grant for the new
//! route. A label-only controller grant from before route keys admits no
//! route.
//!
//! An operator who reviewed an MCP installation before granting it uses
//! [`grant_stock_agent_for_reviewed_installation`]: its grant is issued only
//! while the realm still lists exactly the reviewed descriptor at the
//! reviewed config generation, checked under the config document's writer
//! exclusion held through the selection write. [`grant_stock_agent`] grants
//! the installation as it is current under that exclusion.
//!
//! Replacing a target's grant records the superseded reference for cleanup
//! in the same selection write that selects the new one, and only then
//! retires it, once no unfinished work (attached or durable) references it
//! in any lineage. An explicit revoke is
//! immediate. A new selection takes effect for realms opened afterwards; an
//! open realm's owners keep the selection they were built with.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grants::{GrantRefusal, LocalGrantAuthority};
use meerkat_authorization::mcp_installation::{
    MCP_INSTALLATION_FEATURE, mcp_connector_action, mcp_connector_domain,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{
    ActionRef, DomainRestriction, ExactRestriction, ExecutionRestrictions, LifetimeBound,
    LifetimeRestriction, ProcessorRef, ResourceDomain, SetBound, UnresolvedConstraint,
};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, GrantTarget, TargetGrantLineage, WorkAuthorityBasis,
};
use meerkat_core::AdmissionGuard;
use meerkat_core::connection::RealmId;
use meerkat_core::storage_layout::ResolvedStorage;
use meerkat_core::{ControllerRouteKey, PrincipalKind, PrincipalRef, TrustDomainId};
use serde::{Deserialize, Serialize};

use super::policy::{TOOL_BUNDLE_FEATURE, tool_bundle_namespace};
use super::{StockGovernanceError, StockPrincipals};

/// The agent a stock prompt acts as when none is selected.
pub const DEFAULT_STOCK_AGENT: &str = "rkat-agent";

const STOCK_FEATURE: &str = "stock";
const MCP_FEATURE: &str = "mcp";
const CATALOG_NAMESPACE: &str = "tool-catalog";

/// The operations an MCP server grant allows on its server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOperations {
    /// Exactly these raw operations.
    Exact(BTreeSet<String>),
    /// Every operation of the server.
    All,
}

/// The one target an issued grant covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockGrantTarget {
    /// The agent's controller grant: `stock/infer` on exactly these actual
    /// lowered controller routes.
    Controller {
        routes: BTreeSet<ControllerRouteKey>,
    },
    /// `stock/catalog-read` on the operator's tool catalog.
    Catalog,
    /// One installed MCP server: its entitlement and these operations on its
    /// domain.
    Mcp {
        server: String,
        operations: McpOperations,
    },
    /// Exactly one operation of the tool bundle a trusted host registers
    /// under the stable name `bundle`: `tool_bundle/<operation>` on its
    /// domain. Each operation is its own target, with its own grant,
    /// lifetime and revocation.
    ToolBundleOperation { bundle: String, operation: String },
    /// These raw operations of one installed MCP server, reached only
    /// through the checked route a trusted host registers under the stable
    /// name `route`: the server's entitlement and exactly these operations
    /// on its domain, like an exact MCP server grant, selected for that
    /// route alone. The server's own grant never covers the route, and this
    /// grant never covers the server's ordinary operations. It is route
    /// authority only: it never proves that the host's own checks (its
    /// proposal, approval or consumption) ran for a call.
    CheckedMcpRoute {
        route: String,
        server: String,
        operations: BTreeSet<String>,
    },
}

impl StockGrantTarget {
    pub fn slot(&self) -> StockGrantSlot {
        match self {
            Self::Controller { .. } => StockGrantSlot::Controller,
            Self::Catalog => StockGrantSlot::Catalog,
            Self::Mcp { server, .. } => StockGrantSlot::Mcp(server.clone()),
            Self::ToolBundleOperation { bundle, operation } => {
                StockGrantSlot::ToolBundleOperation {
                    bundle: bundle.clone(),
                    operation: operation.clone(),
                }
            }
            Self::CheckedMcpRoute { route, server, .. } => StockGrantSlot::CheckedMcpRoute {
                route: route.clone(),
                server: server.clone(),
            },
        }
    }
}

/// Where an agent's grant is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockGrantSlot {
    Controller,
    Catalog,
    Mcp(String),
    /// One exact operation of a registered tool bundle.
    ToolBundleOperation {
        bundle: String,
        operation: String,
    },
    /// The checked route a trusted host registers under `route` to the MCP
    /// server `server`.
    CheckedMcpRoute {
        route: String,
        server: String,
    },
    /// The single worker grant of an older selection, covering every target
    /// it was issued for.
    LegacyWorker,
}

impl StockGrantSlot {
    pub fn describe(&self) -> String {
        match self {
            Self::Controller => "controller".to_string(),
            Self::Catalog => "catalog".to_string(),
            Self::Mcp(server) => format!("MCP server '{server}'"),
            Self::ToolBundleOperation { bundle, operation } => {
                format!("operation '{operation}' of tool bundle '{bundle}'")
            }
            Self::CheckedMcpRoute { route, server } => {
                format!("checked route '{route}' to MCP server '{server}'")
            }
            Self::LegacyWorker => "worker".to_string(),
        }
    }
}

/// One explicit issuance request of the operator.
#[derive(Debug, Clone)]
pub struct StockGrantRequest {
    pub agent: String,
    pub target: StockGrantTarget,
    /// The grant's lifetime, from now; unrestricted when `None`. A
    /// controller grant has no expiry (native controller admission requires
    /// an unrestricted lifetime), so one is refused before any effect.
    pub expires_in_ms: Option<u64>,
}

/// What an issuance did: the new grant is issued and selected for its
/// target. The grant it superseded, if any, was recorded for cleanup with
/// that selection; `cleanup` reports every cleanup this command attempted.
#[derive(Debug)]
pub struct StockGrantOutcome {
    pub issued: GrantLineageRef,
    pub superseded: Option<GrantLineageRef>,
    pub cleanup: Vec<CleanupAttempt>,
}

/// What an explicit revoke did, and the cleanup it attempted.
#[derive(Debug)]
pub struct StockRevokeOutcome {
    pub revoked: GrantLineageRef,
    pub outcome: RevocationOutcome,
    /// For a revoked grant: whether its selection entry was removed. When it
    /// was not, the selection still names it and it resolves as revoked.
    pub unselected: Result<(), String>,
    pub cleanup: Vec<CleanupAttempt>,
}

/// One attempt to retire a superseded grant recorded for cleanup.
#[derive(Debug)]
pub struct CleanupAttempt {
    pub agent: String,
    pub grant: GrantLineageRef,
    pub outcome: RevocationOutcome,
}

/// What one revocation attempt did. Only `Revoked` confirms revocation; only
/// `ControllerInUse` and `Deferred` say unfinished work holds the grant. The
/// others report the attempt, not whether the grant is usable now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationOutcome {
    /// The grant owner revoked it.
    Revoked,
    /// Unfinished work retains it in its controller lineage, so it was not
    /// revoked.
    ControllerInUse,
    /// Unfinished work references it, so its cleanup was deferred and not
    /// attempted.
    Deferred,
    /// Whether unfinished work references it could not be determined, so
    /// its cleanup was not attempted.
    ReferencesUnknown,
    /// Controller custody was unavailable, so revocation was not attempted.
    CustodyUnavailable,
    /// The grant owner refused this revocation; it changed nothing.
    Refused,
    /// The grant owner failed during the revocation: it may or may not be
    /// revoked.
    Uncertain,
}

/// Why an issuance did not complete. `Refused` means nothing was issued,
/// selected or revoked; every other case names what is known to exist.
#[derive(Debug, thiserror::Error)]
pub enum StockGrantError {
    #[error(transparent)]
    Refused(#[from] StockGovernanceError),
    /// The grant owner failed while issuing: grant `id` may or may not
    /// exist. The selection is unchanged and nothing was revoked.
    #[error("the issuance of grant {} is uncertain; the selection is unchanged", id.as_str())]
    IssuanceUncertain { id: EvidenceId },
    /// The grant is issued but the selection was not changed: the previous
    /// selection, if any, is preserved.
    #[error("grant {} was issued but not selected: {reason}", issued.grant_id.as_str())]
    NotSelected {
        issued: GrantLineageRef,
        reason: String,
    },
    /// The grant is issued and the selection naming it (with the superseded
    /// grant recorded for cleanup) was written, but its durability is
    /// unknown: after a crash either selection may hold. No cleanup was
    /// attempted.
    #[error(
        "grant {} was issued and selected, but the selection may not be durable: {reason}",
        issued.grant_id.as_str()
    )]
    SelectionUncertain {
        issued: GrantLineageRef,
        previous: Option<GrantLineageRef>,
        reason: String,
    },
    /// The command did not complete: it may already have issued, selected
    /// or revoked.
    #[error("the grant command did not complete; whether it issued or selected is unknown")]
    CommandUncertain,
    /// An admission, or another fenced operation, holds the realm's
    /// admission fence beyond the command's bound: nothing was issued,
    /// selected or revoked. It may be retried once the hold ends, which can
    /// be only when the holding process restarts.
    #[error(
        "an admission or another fenced operation holds the realm's admission fence; nothing \
         was issued, selected or revoked; retry later (an admission's hold may last until its \
         process restarts)"
    )]
    AdmissionInProgress,
    /// The realm's admission fence could not be opened, created, locked or
    /// identity-checked, or the command's guard does not hold it: nothing
    /// was issued, selected or revoked.
    #[error("{0}; nothing was issued, selected or revoked")]
    AdmissionFenceUnavailable(String),
}

impl From<meerkat_core::AdmissionRefusal> for StockGrantError {
    fn from(refusal: meerkat_core::AdmissionRefusal) -> Self {
        match refusal {
            meerkat_core::AdmissionRefusal::InProgress => Self::AdmissionInProgress,
            meerkat_core::AdmissionRefusal::Unavailable(unavailable) => {
                Self::AdmissionFenceUnavailable(unavailable.to_string())
            }
        }
    }
}

/// The executor principal of `agent` in `realm`. It is named by the realm
/// and the agent only; it does not depend on the operating-system user (only
/// the operator is derived from the local user).
pub fn stock_agent_principal(
    realm: &RealmId,
    agent: &str,
) -> Result<PrincipalRef, StockGovernanceError> {
    let domain = TrustDomainId::new(format!("rkat-realm-{}", realm.as_str()))
        .map_err(|error| StockGovernanceError::Principal(error.to_string()))?;
    PrincipalRef::in_domain(PrincipalKind::PersonalAgent, agent, domain)
        .map_err(|error| StockGovernanceError::Principal(error.to_string()))
}

fn domain(operator: &PrincipalRef, namespace: String) -> ResourceDomain {
    ResourceDomain {
        authority: operator.clone(),
        namespace,
    }
}

fn action(feature: &str, action: &str) -> ActionRef {
    ActionRef {
        feature: feature.to_string(),
        action: action.to_string(),
    }
}

fn not_representable(reason: &str) -> StockGovernanceError {
    StockGovernanceError::ScopeNotRepresentable(reason.to_string())
}

/// The exact restrictions of one grant of `target` for `executor`, or the
/// reason it cannot be issued. `now_ms` starts a bounded lifetime. An MCP
/// server grant records `connector`, the connector approval of the server's
/// installation the operator approved at issuance (an explicit no-connector
/// one for a server without a binding): as an exact connector domain always,
/// and as an exact connector action when the operations are exact. A wildcard
/// never approves a connector.
pub fn stock_grant_restrictions(
    operator: &PrincipalRef,
    executor: &PrincipalRef,
    target: &StockGrantTarget,
    connector: Option<&meerkat_core::McpConnectorApproval>,
    expires_in_ms: Option<u64>,
    now_ms: u64,
) -> Result<ExecutionRestrictions, StockGovernanceError> {
    let (actions, resource_domains) = match target {
        StockGrantTarget::Controller { routes } => {
            if routes.is_empty() {
                return Err(not_representable(
                    "a controller grant needs at least one --route",
                ));
            }
            if expires_in_ms.is_some() {
                return Err(not_representable(
                    "a controller grant has no expiry: native controller admission requires an \
                     unrestricted lifetime",
                ));
            }
            (
                ExactRestriction::exact([action(STOCK_FEATURE, "infer")]),
                DomainRestriction::exact(
                    routes
                        .iter()
                        .map(|route| domain(operator, route.as_str().to_string())),
                ),
            )
        }
        StockGrantTarget::Catalog => (
            ExactRestriction::exact([action(STOCK_FEATURE, "catalog-read")]),
            DomainRestriction::exact([domain(operator, CATALOG_NAMESPACE.to_string())]),
        ),
        StockGrantTarget::ToolBundleOperation { bundle, operation } => {
            if bundle.is_empty() || operation.is_empty() {
                return Err(not_representable(
                    "a tool bundle grant needs a bundle name and one operation",
                ));
            }
            (
                ExactRestriction::exact([action(TOOL_BUNDLE_FEATURE, operation)]),
                DomainRestriction::exact([domain(operator, tool_bundle_namespace(bundle))]),
            )
        }
        StockGrantTarget::Mcp { server, operations } => {
            if server.is_empty() {
                return Err(not_representable("an MCP server grant needs a server name"));
            }
            let connector = connector.ok_or_else(|| {
                not_representable("an MCP server grant needs the server's approved installation")
            })?;
            let actions = match operations {
                McpOperations::All => ExactRestriction::unrestricted(),
                McpOperations::Exact(operations) => {
                    if operations.is_empty() {
                        return Err(not_representable("an MCP server grant needs operations"));
                    }
                    let mut actions = vec![
                        action(MCP_INSTALLATION_FEATURE, server),
                        mcp_connector_action(server, connector),
                    ];
                    actions.extend(operations.iter().map(|op| action(MCP_FEATURE, op)));
                    ExactRestriction::exact(actions)
                }
            };
            (
                actions,
                DomainRestriction::exact([
                    domain(operator, format!("mcp-server/{server}")),
                    mcp_connector_domain(operator, server, connector),
                ]),
            )
        }
        // The same exact shape as an MCP server grant of these operations:
        // the route is the target it is selected for, never a widening.
        StockGrantTarget::CheckedMcpRoute {
            route,
            server,
            operations,
        } => {
            if route.is_empty() || server.is_empty() {
                return Err(not_representable(
                    "a checked route grant needs a route name and a server name",
                ));
            }
            if operations.is_empty() || operations.iter().any(String::is_empty) {
                return Err(not_representable(
                    "a checked route grant needs exact operations",
                ));
            }
            let connector = connector.ok_or_else(|| {
                not_representable("a checked route grant needs the server's approved installation")
            })?;
            let mut actions = vec![
                action(MCP_INSTALLATION_FEATURE, server),
                mcp_connector_action(server, connector),
            ];
            actions.extend(operations.iter().map(|op| action(MCP_FEATURE, op)));
            (
                ExactRestriction::exact(actions),
                DomainRestriction::exact([
                    domain(operator, format!("mcp-server/{server}")),
                    mcp_connector_domain(operator, server, connector),
                ]),
            )
        }
    };
    let lifetime = match expires_in_ms {
        None => LifetimeRestriction::unrestricted(),
        Some(ms) => LifetimeRestriction::window(
            now_ms,
            now_ms
                .checked_add(ms)
                .ok_or_else(|| not_representable("the expiry is out of range"))?,
        ),
    };
    Ok(ExecutionRestrictions {
        actions,
        resource_domains,
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: executor.clone(),
        }]),
        lifetime,
        ..ExecutionRestrictions::unrestricted()
    })
}

/// One agent's selected references, per target, and the superseded
/// references awaiting cleanup. References only: the grant owner resolves
/// each at use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentGrantRefs {
    /// The single worker grant of an older selection. It stands in for the
    /// agent's targets only in that older mode: the first per-target grant
    /// moves it to `pending_cleanup` in the same write, so it is never
    /// selected again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<GrantLineageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<GrantLineageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<GrantLineageRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp: BTreeMap<String, GrantLineageRef>,
    /// One grant per exact operation of each registered tool bundle, by the
    /// bundle's stable name, then the operation.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_bundles: BTreeMap<String, BTreeMap<String, GrantLineageRef>>,
    /// One grant per checked route a trusted host registers, by the route's
    /// stable name, then the MCP server it reaches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checked_mcp_routes: BTreeMap<String, BTreeMap<String, GrantLineageRef>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_cleanup: Vec<GrantLineageRef>,
    /// Set once the agent's selection is per target, never unset: the older
    /// single worker grant is then never selected again, whatever slots are
    /// empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub targeted: bool,
}

impl AgentGrantRefs {
    /// Whether the agent is in the older single-worker-grant mode: its
    /// recorded worker grant is present, it never entered per-target mode,
    /// and it selects no per-target grant (a selection holding both is read
    /// as per target). Empty per-target slots alone never select the older
    /// grant.
    fn legacy_only(&self) -> bool {
        !self.targeted && self.operation.is_some() && !self.selects_a_target()
    }

    /// Whether any per-target worker grant (an MCP server, the catalog, a
    /// tool bundle operation or a checked route) is selected.
    fn selects_a_target(&self) -> bool {
        !self.mcp.is_empty()
            || self.catalog.is_some()
            || !self.tool_bundles.is_empty()
            || !self.checked_mcp_routes.is_empty()
    }

    /// Record per-target mode durably once the agent selects a per-target
    /// grant (including a selection written before the mode existed that
    /// holds both): the older worker grant is then recorded for cleanup and
    /// never selected again.
    fn settle_mode(&mut self) {
        if self.selects_a_target() {
            self.targeted = true;
        }
        if self.targeted
            && let Some(legacy) = self.operation.take()
        {
            self.pending_cleanup.push(legacy);
        }
    }

    fn selects(&self, grant: &GrantLineageRef) -> bool {
        self.operation.as_ref() == Some(grant)
            || self.controller.as_ref() == Some(grant)
            || self.catalog.as_ref() == Some(grant)
            || self.mcp.values().any(|selected| selected == grant)
            || self
                .tool_bundles
                .values()
                .flat_map(BTreeMap::values)
                .any(|selected| selected == grant)
            || self
                .checked_mcp_routes
                .values()
                .flat_map(BTreeMap::values)
                .any(|selected| selected == grant)
    }

    /// The exact worker lineage selected for configured server `server`: its
    /// own grant, or for an older selection its single worker grant.
    pub fn server_lineage(&self, server: &str) -> Option<&GrantLineageRef> {
        if self.legacy_only() {
            self.operation.as_ref()
        } else {
            self.mcp.get(server)
        }
    }

    /// The work basis and controller lineage of the agent's prompts, or
    /// `None` until the operator selected its controller and a worker grant.
    /// An older selection's single worker grant is the whole basis; otherwise
    /// each MCP server's grant is that server's target lineage, each tool
    /// bundle operation's grant that exact operation's target lineage, each
    /// checked route's grant that exact route's target lineage, and the
    /// catalog grant (if any) the base. A server, bundle operation or checked
    /// route with no grant of its own has no lineage: its operations are
    /// refused, never served by another grant (the catalog base and a
    /// server's ordinary grant included).
    pub fn prompt_selection(&self) -> Option<(WorkAuthorityBasis, GrantLineageRef)> {
        let controller = self.controller.clone()?;
        let basis = if self.legacy_only() {
            WorkAuthorityBasis::GrantLineage {
                lineage: vec![self.operation.clone()?],
            }
        } else {
            let servers = self.mcp.iter().map(|(server, lineage)| TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: server.clone(),
                },
                lineage: vec![lineage.clone()],
            });
            let bundle_operations = self.tool_bundles.iter().flat_map(|(bundle, operations)| {
                operations
                    .iter()
                    .map(move |(operation, lineage)| TargetGrantLineage {
                        target: GrantTarget::RegisteredToolBundleOperation {
                            bundle: bundle.clone(),
                            operation: operation.clone(),
                        },
                        lineage: vec![lineage.clone()],
                    })
            });
            let checked_routes = self.checked_mcp_routes.iter().flat_map(|(route, servers)| {
                servers
                    .iter()
                    .map(move |(server, lineage)| TargetGrantLineage {
                        target: GrantTarget::CheckedMcpRoute {
                            route: route.clone(),
                            server: server.clone(),
                        },
                        lineage: vec![lineage.clone()],
                    })
            });
            // One merged selection in the canonical target order.
            let mut targets: Vec<TargetGrantLineage> = servers
                .chain(bundle_operations)
                .chain(checked_routes)
                .collect();
            targets.sort_by(|left, right| left.target.cmp(&right.target));
            WorkAuthorityBasis::TargetedGrantLineages {
                base: self.catalog.clone().map(|catalog| vec![catalog]),
                targets,
            }
        };
        Some((basis, controller))
    }

    pub fn get(&self, slot: &StockGrantSlot) -> Option<&GrantLineageRef> {
        match slot {
            StockGrantSlot::Controller => self.controller.as_ref(),
            StockGrantSlot::Catalog => self.catalog.as_ref(),
            StockGrantSlot::Mcp(server) => self.mcp.get(server),
            StockGrantSlot::ToolBundleOperation { bundle, operation } => self
                .tool_bundles
                .get(bundle)
                .and_then(|operations| operations.get(operation)),
            StockGrantSlot::CheckedMcpRoute { route, server } => self
                .checked_mcp_routes
                .get(route)
                .and_then(|servers| servers.get(server)),
            StockGrantSlot::LegacyWorker => self.operation.as_ref(),
        }
    }

    /// The lineage this selection names for `slot` as the agent's prompts use
    /// it ([`Self::prompt_selection`]), and where it comes from: in the older
    /// mode the single worker grant serves every worker slot, the controller
    /// slot is always its own, and a reference that only awaits cleanup is
    /// never selected.
    fn selected_for(
        &self,
        slot: &StockGrantSlot,
    ) -> Option<(&GrantLineageRef, StockSelectionSource)> {
        match slot {
            StockGrantSlot::Controller => self
                .controller
                .as_ref()
                .map(|lineage| (lineage, StockSelectionSource::PerTarget)),
            _ if self.legacy_only() => self
                .operation
                .as_ref()
                .map(|lineage| (lineage, StockSelectionSource::LegacyWorker)),
            StockGrantSlot::LegacyWorker => None,
            StockGrantSlot::Catalog
            | StockGrantSlot::Mcp(_)
            | StockGrantSlot::ToolBundleOperation { .. }
            | StockGrantSlot::CheckedMcpRoute { .. } => self
                .get(slot)
                .map(|lineage| (lineage, StockSelectionSource::PerTarget)),
        }
    }

    fn replace(
        &mut self,
        slot: &StockGrantSlot,
        grant: GrantLineageRef,
    ) -> Option<GrantLineageRef> {
        match slot {
            StockGrantSlot::Controller => self.controller.replace(grant),
            StockGrantSlot::Catalog => self.catalog.replace(grant),
            StockGrantSlot::Mcp(server) => self.mcp.insert(server.clone(), grant),
            StockGrantSlot::ToolBundleOperation { bundle, operation } => self
                .tool_bundles
                .entry(bundle.clone())
                .or_default()
                .insert(operation.clone(), grant),
            StockGrantSlot::CheckedMcpRoute { route, server } => self
                .checked_mcp_routes
                .entry(route.clone())
                .or_default()
                .insert(server.clone(), grant),
            StockGrantSlot::LegacyWorker => self.operation.replace(grant),
        }
    }

    fn take(&mut self, slot: &StockGrantSlot) -> Option<GrantLineageRef> {
        match slot {
            StockGrantSlot::Controller => self.controller.take(),
            StockGrantSlot::Catalog => self.catalog.take(),
            StockGrantSlot::Mcp(server) => self.mcp.remove(server),
            StockGrantSlot::ToolBundleOperation { bundle, operation } => {
                let operations = self.tool_bundles.get_mut(bundle)?;
                let taken = operations.remove(operation);
                if operations.is_empty() {
                    self.tool_bundles.remove(bundle);
                }
                taken
            }
            StockGrantSlot::CheckedMcpRoute { route, server } => {
                let servers = self.checked_mcp_routes.get_mut(route)?;
                let taken = servers.remove(server);
                if servers.is_empty() {
                    self.checked_mcp_routes.remove(route);
                }
                taken
            }
            StockGrantSlot::LegacyWorker => self.operation.take(),
        }
    }
}

/// The realm's grant selection: references per agent, nothing else.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct StockGrantFile {
    pub agents: BTreeMap<String, AgentGrantRefs>,
}

impl StockGrantFile {
    /// Whether any agent still selects `grant` in any slot.
    fn selects(&self, grant: &GrantLineageRef) -> bool {
        self.agents.values().any(|refs| refs.selects(grant))
    }

    /// Settle every agent's mode before a write, and drop from cleanup any
    /// reference still selected somewhere: a selected grant is never
    /// retired.
    pub(super) fn settle(&mut self) {
        for refs in self.agents.values_mut() {
            refs.settle_mode();
        }
        let selected = self.clone();
        for refs in self.agents.values_mut() {
            refs.pending_cleanup
                .retain(|grant| !selected.selects(grant));
            let mut seen = Vec::new();
            refs.pending_cleanup.retain(|grant| {
                let first = !seen.contains(grant);
                seen.push(grant.clone());
                first
            });
        }
    }
}

impl<'de> Deserialize<'de> for StockGrantFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Persisted {
            PerAgent {
                agents: BTreeMap<String, AgentGrantRefs>,
            },
            // The single-agent form names the default agent's pair.
            Legacy {
                operation: GrantLineageRef,
                controller: GrantLineageRef,
            },
        }
        Ok(match Persisted::deserialize(deserializer)? {
            Persisted::PerAgent { agents } => Self { agents },
            Persisted::Legacy {
                operation,
                controller,
            } => Self {
                agents: BTreeMap::from([(
                    DEFAULT_STOCK_AGENT.to_string(),
                    AgentGrantRefs {
                        operation: Some(operation),
                        controller: Some(controller),
                        ..AgentGrantRefs::default()
                    },
                )]),
            },
        })
    }
}

pub(super) fn restore_owner(
    realms_root: &Path,
    realm_id: &str,
) -> Result<Arc<LocalGrantAuthority>, StockGovernanceError> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    let store = Arc::new(super::super::realm_grant_owner_store(&paths)?);
    LocalGrantAuthority::restore(
        store,
        LocalAuthorizationPublication::new(),
        Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
    )
    .map(Arc::new)
    .map_err(|error| StockGovernanceError::GrantOwner(error.to_string()))
}

/// Hold the governed realm's execution custody for an administrative act.
pub(super) async fn administrative_custody(
    realms_root: &Path,
    realm_id: &str,
) -> Result<crate::PersistenceBundle, StockGovernanceError> {
    if meerkat_store::read_realm_posture_in(realms_root, realm_id).await?
        != meerkat_store::RealmPosture::Governed
    {
        return Err(StockGovernanceError::NotGoverned);
    }
    let (_, bundle) = super::super::open_governed_realm_persistence_with_provider(
        &crate::DiskStorageProvider,
        realms_root,
        realm_id,
        None,
        None,
        None,
        super::super::RealmGovernance::GovernedWithoutOwners,
    )
    .await?;
    Ok(bundle)
}

fn custody_refusal_outcome(refusal: ControllerCustodyRefusal) -> RevocationOutcome {
    match refusal {
        ControllerCustodyRefusal::ControllerInUse => RevocationOutcome::ControllerInUse,
        ControllerCustodyRefusal::Unavailable => RevocationOutcome::CustodyUnavailable,
    }
}

/// A generated refusal precedes every owner change; an unavailable owner may
/// fail after its durable append, so the revocation is then uncertain. A
/// guard that does not hold the realm's fence is refused before anything is
/// appended.
fn revocation_outcome(result: Result<(), GrantRefusal>) -> RevocationOutcome {
    match result {
        Ok(()) => RevocationOutcome::Revoked,
        Err(GrantRefusal::ControllerInUse) => RevocationOutcome::ControllerInUse,
        // Duplicate is an issuance refusal; a revocation never returns it.
        Err(GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::AdmissionFence) => {
            RevocationOutcome::Refused
        }
        Err(GrantRefusal::Unavailable) => RevocationOutcome::Uncertain,
    }
}

/// An administrative process admits no originals, so it cannot say whether
/// an original that unfinished work names as a contributor still retains a
/// grant: such work makes the reference query unavailable, and the cleanup
/// is deferred rather than risked.
struct NoRetainedOriginals;

impl meerkat_runtime::meerkat_machine::AuthenticatedOriginalSource for NoRetainedOriginals {
    fn authenticated_original(
        &self,
        _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
        _namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _original: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        Option<meerkat_runtime::meerkat_machine::AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)
    }

    fn retained_custody(
        &self,
        _namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        _original: &meerkat_authorization_contracts::work_association::OriginalWorkRef,
    ) -> Result<
        meerkat_runtime::meerkat_machine::RetainedCustody,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)
    }
}

/// The effects of the operator's grant commands, in order. Separate so each
/// failure point is reported for what it leaves behind.
trait IssuanceEffects {
    fn issue(
        &self,
        id: EvidenceId,
        executor: PrincipalRef,
        restrictions: ExecutionRestrictions,
    ) -> Result<GrantLineageRef, GrantRefusal>;

    async fn select(&self, file: &StockGrantFile) -> Result<(), super::SelectionWriteFailure>;

    /// Retire a superseded grant: revoke it only when no unfinished work
    /// references it, under one custody.
    fn retire(&self, grant: &GrantLineageRef) -> RevocationOutcome;

    /// Revoke a selected grant now (the native controller protection still
    /// applies).
    fn revoke(&self, grant: &GrantLineageRef) -> RevocationOutcome;
}

struct RealmEffects<'a> {
    owner: &'a LocalGrantAuthority,
    machine: &'a meerkat_runtime::MeerkatMachine,
    operator: &'a PrincipalRef,
    paths: &'a meerkat_store::RealmPaths,
    /// The command's guard: it holds the realm's admission fence, which
    /// covers the grant log and the selection.
    guard: &'a AdmissionGuard,
}

impl IssuanceEffects for RealmEffects<'_> {
    fn issue(
        &self,
        id: EvidenceId,
        executor: PrincipalRef,
        restrictions: ExecutionRestrictions,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        self.owner
            .issue_root(self.guard, self.operator, id, executor, None, restrictions)
    }

    async fn select(&self, file: &StockGrantFile) -> Result<(), super::SelectionWriteFailure> {
        #[cfg(test)]
        held_publication(&self.paths.root).await;
        super::write_stock_grant_file(self.guard, self.paths, file).await
    }

    fn retire(&self, grant: &GrantLineageRef) -> RevocationOutcome {
        let mut custody = match self.machine.try_controller_grant_mutation() {
            Ok(custody) => custody,
            Err(refusal) => return custody_refusal_outcome(refusal),
        };
        // The reference query and the revocation run under the same held
        // custody, so no work can take the grant up in between.
        match custody.revoke_if_unreferenced(grant, &NoRetainedOriginals, |custody| {
            self.owner.revoke(self.guard, self.operator, grant, custody)
        }) {
            Ok(Some(result)) => revocation_outcome(result),
            Ok(None) => RevocationOutcome::Deferred,
            Err(ControllerCustodyRefusal::ControllerInUse) => RevocationOutcome::ControllerInUse,
            Err(ControllerCustodyRefusal::Unavailable) => RevocationOutcome::ReferencesUnknown,
        }
    }

    fn revoke(&self, grant: &GrantLineageRef) -> RevocationOutcome {
        match self.machine.try_controller_grant_mutation() {
            Ok(mut custody) => revocation_outcome(self.owner.revoke(
                self.guard,
                self.operator,
                grant,
                &mut custody,
            )),
            Err(refusal) => custody_refusal_outcome(refusal),
        }
    }
}

/// Try to retire every reference recorded for cleanup. Retired references
/// leave the record in a second write; if that write fails they stay listed
/// and a later retirement of an already revoked grant is a no-op.
async fn run_cleanup(
    effects: &impl IssuanceEffects,
    file: &mut StockGrantFile,
) -> Vec<CleanupAttempt> {
    let mut attempts = Vec::new();
    for (agent, refs) in &file.agents {
        for grant in &refs.pending_cleanup {
            if file.selects(grant) {
                continue;
            }
            attempts.push(CleanupAttempt {
                agent: agent.clone(),
                grant: grant.clone(),
                outcome: effects.retire(grant),
            });
        }
    }
    let retired: Vec<&CleanupAttempt> = attempts
        .iter()
        .filter(|attempt| attempt.outcome == RevocationOutcome::Revoked)
        .collect();
    if !retired.is_empty() {
        for attempt in &retired {
            if let Some(refs) = file.agents.get_mut(&attempt.agent) {
                refs.pending_cleanup.retain(|grant| grant != &attempt.grant);
            }
        }
        let _ = effects.select(file).await;
    }
    attempts
}

/// Issue, select the issued grant for its target in `file` (the selection
/// read before any effect) together with the superseded grant recorded for
/// cleanup, then attempt the recorded cleanups. Not a transaction: each
/// failure reports what it leaves behind, and a selection that may not be
/// durable attempts no cleanup.
async fn issue_and_select(
    effects: &impl IssuanceEffects,
    mut file: StockGrantFile,
    agent: &str,
    slot: &StockGrantSlot,
    id: EvidenceId,
    executor: PrincipalRef,
    restrictions: ExecutionRestrictions,
) -> Result<StockGrantOutcome, StockGrantError> {
    let issued = match effects.issue(id.clone(), executor, restrictions) {
        Ok(issued) => issued,
        Err(GrantRefusal::Unavailable) => return Err(StockGrantError::IssuanceUncertain { id }),
        // Refused before anything was appended.
        Err(GrantRefusal::AdmissionFence) => {
            return Err(StockGrantError::AdmissionFenceUnavailable(
                "the command does not hold the realm's admission fence".to_string(),
            ));
        }
        Err(refusal) => {
            return Err(StockGovernanceError::GrantOwner(refusal.to_string()).into());
        }
    };
    let entry = file.agents.entry(agent.to_string()).or_default();
    let previous = entry
        .replace(slot, issued.clone())
        .filter(|previous| previous != &issued);
    if let Some(previous) = &previous {
        entry.pending_cleanup.push(previous.clone());
    }
    // Adopting a per-target grant ends the older mode for good, in this
    // same write; a superseded reference still selected elsewhere is not
    // queued for cleanup.
    file.settle();
    match effects.select(&file).await {
        Ok(()) => {}
        Err(super::SelectionWriteFailure::NotWritten(error)) => {
            return Err(StockGrantError::NotSelected {
                issued,
                reason: error.to_string(),
            });
        }
        Err(super::SelectionWriteFailure::NotDurable(error)) => {
            return Err(StockGrantError::SelectionUncertain {
                issued,
                previous,
                reason: error.to_string(),
            });
        }
    }
    let cleanup = run_cleanup(effects, &mut file).await;
    Ok(StockGrantOutcome {
        issued,
        superseded: previous,
        cleanup,
    })
}

/// Revoke the grant selected in `slot` now; once revoked, remove it from the
/// selection. Then attempt the recorded cleanups.
async fn revoke_and_unselect(
    effects: &impl IssuanceEffects,
    mut file: StockGrantFile,
    agent: &str,
    slot: &StockGrantSlot,
) -> Result<StockRevokeOutcome, StockGovernanceError> {
    // A selection written before the per-target mode existed is settled
    // first, so removing its last per-target grant never selects the older
    // worker grant again.
    file.settle();
    let revoked = file
        .agents
        .get(agent)
        .and_then(|refs| refs.get(slot))
        .cloned()
        .ok_or(StockGovernanceError::GrantsUnavailable)?;
    let outcome = effects.revoke(&revoked);
    let mut unselected = Ok(());
    if outcome == RevocationOutcome::Revoked {
        if let Some(refs) = file.agents.get_mut(agent) {
            refs.take(slot);
        }
        file.settle();
        unselected = effects
            .select(&file)
            .await
            .map_err(|failure| StockGovernanceError::from(failure).to_string());
    }
    let cleanup = run_cleanup(effects, &mut file).await;
    Ok(StockRevokeOutcome {
        revoked,
        outcome,
        unselected,
        cleanup,
    })
}

/// Issue `request` as the realm's operator and select it for its target.
///
/// The target and the current selection are validated before any effect;
/// then issue, write the selection naming the new grant with the superseded
/// one recorded for cleanup, and attempt the recorded cleanups (see
/// [`StockGrantError`] and [`RevocationOutcome`]).
///
/// The command's owned task first takes the realm's admission fence
/// (`meerkat_core::realm_admission`), waiting up to the interactive bound,
/// before any custody: a held fence refuses
/// ([`StockGrantError::AdmissionInProgress`]) with nothing issued, selected
/// or revoked.
pub async fn grant_stock_agent(
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
) -> Result<StockGrantOutcome, StockGrantError> {
    let (realms_root, realm_id) = (realms_root.to_path_buf(), realm_id.to_string());
    let (principals, request) = (principals.clone(), request.clone());
    on_owned_task(async move {
        let guard = super::take_realm_fence::<StockGrantError>(&realms_root, &realm_id).await?;
        grant_under_custody(&guard, &realms_root, &realm_id, &principals, &request).await
    })
    .await
}

/// The exact MCP installation an operator reviewed before granting it: the
/// full server descriptor the realm's own config document listed, and that
/// document's generation when it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedMcpInstallation {
    pub server: meerkat_core::McpServerConfig,
    pub generation: u64,
}

/// [`grant_stock_agent`] for an MCP server grant whose installation the
/// operator reviewed: the grant is issued for exactly `reviewed`, or not at
/// all.
///
/// `request` must be an MCP target, or a checked route target, naming
/// `reviewed.server`; any other target is refused before any custody or
/// effect. The realm's own config
/// document is `storage`'s, by the realm-to-document projection the reviewed
/// connector finalizer shares: for the reserved `global` realm the
/// user-global document, never a shadow under the state root (a global
/// realm whose
/// layout gives that document no location is refused as unavailable before
/// any effect). The grant is prepared with the reviewed installation's
/// connector approval. Then, on the command's owned task, under the realm's
/// admission fence (taken first, as [`grant_stock_agent`] takes it), its
/// administrative custody and then that document's writer exclusion (held
/// through the selection write), the document must still list exactly
/// `reviewed.server` under its name at `reviewed.generation`; otherwise
/// nothing is issued or selected
/// ([`StockGovernanceError::InstallationChanged`]). Past that check every
/// outcome, including an uncertain one, is [`grant_stock_agent`]'s.
pub async fn grant_stock_agent_for_reviewed_installation(
    storage: &ResolvedStorage,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
    reviewed: &ReviewedMcpInstallation,
) -> Result<StockGrantOutcome, StockGrantError> {
    match &request.target {
        StockGrantTarget::Mcp { server, .. } | StockGrantTarget::CheckedMcpRoute { server, .. }
            if *server == reviewed.server.name => {}
        _ => {
            return Err(not_representable(
                "a reviewed installation grant needs an MCP or checked route target naming the \
                 reviewed server",
            )
            .into());
        }
    }
    let document = realm_config_source(storage)
        .ok_or(StockGovernanceError::ConfigurationUnavailable)?
        .config_doc_path(&storage.locator.realm);
    let realm_id = storage.locator.realm.as_str().to_string();
    let prepared = prepare_grant(
        &realm_id,
        principals,
        request,
        Some(&reviewed.server.connector_approval()),
    )?;
    let realms_root = storage.locator.state_root.clone();
    let (principals, request, reviewed) = (principals.clone(), request.clone(), reviewed.clone());
    on_owned_task(async move {
        let guard = super::take_realm_fence::<StockGrantError>(&realms_root, &realm_id).await?;
        let bundle = administrative_custody(&realms_root, &realm_id).await?;
        let outcome = issue_for_current_installation(
            &bundle,
            &guard,
            &realms_root,
            &realm_id,
            &principals,
            &request,
            prepared,
            &document,
            &reviewed.server,
            reviewed.generation,
        )
        .await;
        drop(bundle);
        outcome
    })
    .await
}

/// The realm's config source with the shared realm-to-document projection:
/// the reserved `global` realm's own document is the user-global document,
/// never a shadow under the state root. `None` for the `global` realm when
/// the layout gives the user-global document no location.
pub(crate) fn realm_config_source(
    storage: &ResolvedStorage,
) -> Option<meerkat_store::FilesystemRealmConfigSource> {
    let global = storage.layout.global_config_path();
    if storage.locator.realm.is_global() && global.is_none() {
        return None;
    }
    Some(meerkat_store::FilesystemRealmConfigSource::new(
        storage.locator.state_root.clone(),
        global.unwrap_or_default(),
        meerkat_models::canonical(),
    ))
}

/// Run one administrative command on its own task: the realm's
/// administrative custody, and every effect entered under it (an issued
/// grant, a selection write and its publication, a revocation), settles
/// even when the caller stops waiting. Custody is never released while an
/// entered effect can still land.
///
/// A command task that did not complete (it panicked, or the runtime shut
/// down under it) may already have issued, selected or revoked: that is
/// reported as an uncertain outcome, never as a refusal.
pub(super) async fn on_owned_task<T, E>(
    command: impl std::future::Future<Output = Result<T, E>> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static + UncertainCommand,
{
    tokio::spawn(command)
        .await
        .unwrap_or_else(|_| Err(E::uncertain()))
}

/// An administrative command's error for an outcome that is not known.
pub(super) trait UncertainCommand {
    fn uncertain() -> Self;
}

impl UncertainCommand for StockGovernanceError {
    fn uncertain() -> Self {
        Self::CommandUncertain
    }
}

impl UncertainCommand for StockGrantError {
    fn uncertain() -> Self {
        Self::CommandUncertain
    }
}

/// A test hold on an entered selection write of one realm (keyed by its
/// root): that command acknowledges that it holds custody and is about to
/// publish, then waits for the release. Writes of other realms pass.
#[cfg(test)]
pub(in crate::governed_realm) type PublicationHold = (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// The armed holds, at most one per realm root: concurrent tests each hold
/// their own realm and never replace another test's hold.
#[cfg(test)]
pub(in crate::governed_realm) static PUBLICATION_HOLDS: std::sync::Mutex<Vec<PublicationHold>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(super) async fn held_publication(realm_root: &Path) {
    let hold = PUBLICATION_HOLDS.lock().ok().and_then(|mut holds| {
        holds
            .iter()
            .position(|(root, _, _)| root == realm_root)
            .map(|index| holds.remove(index))
    });
    if let Some((_, acknowledge, release)) = hold {
        let _ = acknowledge.send(());
        let _ = release.await;
    }
}

/// [`grant_stock_agent`] on its owned task: a target that needs no
/// installation (a controller, the catalog or a tool bundle operation) is
/// prepared, and an
/// unrepresentable one (a controller expiry) refused, before any custody or
/// effect. Under the realm's administrative custody an MCP server grant
/// then reads the installation it approves under the config document's
/// writer exclusion and is prepared from it; that exclusion and the
/// custody are held until the selection naming the issued grant is
/// written. `guard` holds the realm's admission fence.
async fn grant_under_custody(
    guard: &AdmissionGuard,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
) -> Result<StockGrantOutcome, StockGrantError> {
    let without_installation = prepare_without_installation(realm_id, principals, request)?;
    let bundle = administrative_custody(realms_root, realm_id).await?;
    let machine = bundle.runtime_adapter();
    let outcome = grant_prepared_under(
        &machine,
        guard,
        without_installation,
        realms_root,
        realm_id,
        principals,
        request,
    )
    .await;
    drop(machine);
    drop(bundle);
    outcome
}

/// [`grant_stock_agent`]'s command under the execution custody of a realm's
/// staged governed machine (see [`crate::governed_realm::staged`]) instead
/// of a separately opened administrative custody: `machine` is a clone of
/// that stage, which already excludes every other opener. Run on the
/// stage's owned task. The realm's admission fence is taken first, as
/// [`grant_stock_agent`] takes it, but without waiting, since the stage's
/// custody is already held: an admission's hold refuses at once with
/// [`StockGrantError::AdmissionInProgress`]. The same validation precedes
/// every effect.
pub(crate) async fn grant_under_staged_custody(
    machine: meerkat_runtime::MeerkatMachine,
    realms_root: std::path::PathBuf,
    realm_id: String,
    principals: StockPrincipals,
    request: StockGrantRequest,
) -> Result<StockGrantOutcome, StockGrantError> {
    let guard = super::take_realm_fence_now::<StockGrantError>(&realms_root, &realm_id)?;
    let without_installation = prepare_without_installation(&realm_id, &principals, &request)?;
    grant_prepared_under(
        &machine,
        &guard,
        without_installation,
        &realms_root,
        &realm_id,
        &principals,
        &request,
    )
    .await
}

/// A target that needs no installation is prepared before any custody or
/// effect; an MCP target (a checked route included) is prepared later, from
/// the installation it approves.
fn prepare_without_installation(
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
) -> Result<Option<PreparedGrant>, StockGovernanceError> {
    match &request.target {
        StockGrantTarget::Mcp { .. } | StockGrantTarget::CheckedMcpRoute { .. } => Ok(None),
        StockGrantTarget::Controller { .. }
        | StockGrantTarget::Catalog
        | StockGrantTarget::ToolBundleOperation { .. } => {
            prepare_grant(realm_id, principals, request, None).map(Some)
        }
    }
}

/// The custody-held part of [`grant_under_custody`]: under the realm
/// execution custody `machine` holds, an MCP server grant reads the
/// installation it approves under the config document's writer exclusion,
/// held until the selection naming the issued grant is written.
async fn grant_prepared_under(
    machine: &meerkat_runtime::MeerkatMachine,
    guard: &AdmissionGuard,
    without_installation: Option<PreparedGrant>,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
) -> Result<StockGrantOutcome, StockGrantError> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    // The operator approves the server's installation as the realm records
    // it now, under the config document's writer exclusion held until the
    // selection is written: no config change interleaves, and the binding
    // is recorded in the grant, never filled in later from another
    // configuration.
    let (_config_exclusion, prepared) = match (without_installation, &request.target) {
        (Some(prepared), _) => (None, prepared),
        (
            None,
            StockGrantTarget::Mcp { server, .. } | StockGrantTarget::CheckedMcpRoute { server, .. },
        ) => {
            let (exclusion, installed) =
                super::super::installation_under_writer_exclusion(&paths, server)
                    .map_err(|_| StockGovernanceError::ConfigurationUnavailable)?;
            let installed = installed.ok_or_else(|| {
                not_representable(&format!(
                    "'{server}' is not one of the realm's own MCP installations"
                ))
            })?;
            let approval = installed.connector_approval();
            (
                Some(exclusion),
                prepare_grant(realm_id, principals, request, Some(&approval))?,
            )
        }
        (
            None,
            StockGrantTarget::Controller { .. }
            | StockGrantTarget::Catalog
            | StockGrantTarget::ToolBundleOperation { .. },
        ) => (None, prepare_grant(realm_id, principals, request, None)?),
    };
    issue_prepared_with_machine(
        machine,
        guard,
        realms_root,
        realm_id,
        principals,
        request,
        prepared,
    )
    .await
}

/// A grant validated before any effect: its executor, exact restrictions and
/// fresh id.
pub(super) struct PreparedGrant {
    executor: PrincipalRef,
    restrictions: ExecutionRestrictions,
    id: EvidenceId,
}

/// Validate `request` for `realm_id` and prepare its grant, with no effect.
/// An MCP server grant records `connector`, the connector approval of the
/// installation the operator approves ([`stock_grant_restrictions`]).
pub(super) fn prepare_grant(
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
    connector: Option<&meerkat_core::McpConnectorApproval>,
) -> Result<PreparedGrant, StockGovernanceError> {
    let realm = RealmId::parse(realm_id)
        .map_err(|error| StockGovernanceError::Principal(error.to_string()))?;
    let executor = stock_agent_principal(&realm, &request.agent)?;
    let now_ms = meerkat_authorization::clock::HostAuthorizationClock
        .now()
        .map_err(|_| StockGovernanceError::GrantOwner("the clock is unavailable".to_string()))?
        .unix_ms;
    let restrictions = stock_grant_restrictions(
        &principals.operator,
        &executor,
        &request.target,
        connector,
        request.expires_in_ms,
        now_ms,
    )?;
    let kind = match &request.target {
        StockGrantTarget::Controller { .. } => "controller".to_string(),
        StockGrantTarget::Catalog => "catalog".to_string(),
        StockGrantTarget::Mcp { server, .. } => format!("mcp-{server}"),
        StockGrantTarget::ToolBundleOperation { bundle, operation } => {
            format!("bundle-{bundle}-{operation}")
        }
        StockGrantTarget::CheckedMcpRoute { route, server, .. } => {
            format!("route-{route}-{server}")
        }
    };
    let id = EvidenceId::new(format!("{}-{kind}-{}", request.agent, uuid::Uuid::now_v7()))
        .map_err(|error| StockGovernanceError::GrantOwner(error.to_string()))?;
    Ok(PreparedGrant {
        executor,
        restrictions,
        id,
    })
}

/// Issue and select `prepared` for `request`'s target under the
/// administrative custody `bundle` the caller holds (see
/// [`grant_stock_agent`]) and the realm's admission fence `guard` holds.
pub(super) async fn issue_prepared_in_custody(
    bundle: &crate::PersistenceBundle,
    guard: &AdmissionGuard,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
    prepared: PreparedGrant,
) -> Result<StockGrantOutcome, StockGrantError> {
    let machine = bundle.runtime_adapter();
    issue_prepared_with_machine(
        &machine,
        guard,
        realms_root,
        realm_id,
        principals,
        request,
        prepared,
    )
    .await
}

/// [`issue_prepared_in_custody`] under the realm execution custody `machine`
/// holds (an administrative bundle's machine or a staged realm's).
async fn issue_prepared_with_machine(
    machine: &meerkat_runtime::MeerkatMachine,
    guard: &AdmissionGuard,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
    prepared: PreparedGrant,
) -> Result<StockGrantOutcome, StockGrantError> {
    let owner = restore_owner(realms_root, realm_id)?;
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    let file = super::read_stock_grant_file(&paths)
        .await?
        .unwrap_or_default();
    let effects = RealmEffects {
        owner: &owner,
        machine,
        operator: &principals.operator,
        paths: &paths,
        guard,
    };
    issue_and_select(
        &effects,
        file,
        &request.agent,
        &request.target.slot(),
        prepared.id,
        prepared.executor,
        prepared.restrictions,
    )
    .await
}

/// Issue and select `prepared` for `request`'s MCP target under the
/// administrative custody `bundle` the caller holds and the config document
/// `document`'s writer exclusion, taken after that custody (as every
/// administrative command takes them) and held through the selection write,
/// only while the document still lists exactly `server` under its name at
/// `generation`. Otherwise the grant is refused locally
/// ([`StockGovernanceError::InstallationChanged`]) and nothing is issued or
/// selected; a generation that cannot be read is unavailable. `guard` holds
/// the realm's admission fence, taken before the custody; the writer
/// exclusion is the document's config custody alone and takes no fence.
#[allow(clippy::too_many_arguments)]
pub(super) async fn issue_for_current_installation(
    bundle: &crate::PersistenceBundle,
    guard: &AdmissionGuard,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    request: &StockGrantRequest,
    prepared: PreparedGrant,
    document: &Path,
    server: &meerkat_core::McpServerConfig,
    generation: u64,
) -> Result<StockGrantOutcome, StockGrantError> {
    let (_exclusion, installed, current) =
        super::super::installation_under_writer_exclusion_in(document, &server.name)
            .map_err(|_| StockGovernanceError::ConfigurationUnavailable)?;
    // An unreadable generation is not known to have moved: unavailable.
    let current = current.ok_or(StockGovernanceError::ConfigurationUnavailable)?;
    if installed.as_ref() != Some(server) || current != generation {
        return Err(StockGovernanceError::InstallationChanged(server.name.clone()).into());
    }
    issue_prepared_in_custody(
        bundle,
        guard,
        realms_root,
        realm_id,
        principals,
        request,
        prepared,
    )
    .await
}

/// Revoke an agent's grant selected in `slot` now, through the grant owner
/// under native controller custody, and remove it from the selection once
/// revoked. The realm's admission fence is taken first, as
/// [`grant_stock_agent`] takes it: a held fence refuses
/// ([`StockGovernanceError::AdmissionInProgress`]) with nothing revoked.
pub async fn revoke_stock_agent(
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    agent: &str,
    slot: &StockGrantSlot,
) -> Result<StockRevokeOutcome, StockGovernanceError> {
    let (realms_root, realm_id) = (realms_root.to_path_buf(), realm_id.to_string());
    let (principals, agent, slot) = (principals.clone(), agent.to_string(), slot.clone());
    on_owned_task(async move {
        let guard =
            super::take_realm_fence::<StockGovernanceError>(&realms_root, &realm_id).await?;
        revoke_under_custody(&guard, &realms_root, &realm_id, &principals, &agent, &slot).await
    })
    .await
}

async fn revoke_under_custody(
    guard: &AdmissionGuard,
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
    agent: &str,
    slot: &StockGrantSlot,
) -> Result<StockRevokeOutcome, StockGovernanceError> {
    let bundle = administrative_custody(realms_root, realm_id).await?;
    let owner = restore_owner(realms_root, realm_id)?;
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    let file = super::read_stock_grant_file(&paths)
        .await?
        .ok_or(StockGovernanceError::GrantsUnavailable)?;
    let machine = bundle.runtime_adapter();
    let effects = RealmEffects {
        owner: &owner,
        machine: &machine,
        operator: &principals.operator,
        paths: &paths,
        guard,
    };
    let outcome = revoke_and_unselect(&effects, file, agent, slot).await;
    drop(bundle);
    outcome
}

/// Whose selection an observation reads: the agent, and the executor its
/// lineages are resolved for.
pub(super) struct ObservedAgent {
    realm: RealmId,
    agent: String,
    executor: PrincipalRef,
}

impl ObservedAgent {
    /// The named agent of `realm` with the executor stock derives for it
    /// ([`stock_agent_principal`]): the offline and staged reads.
    pub(super) fn new(realm: RealmId, agent: &str) -> Result<Self, StockGovernanceError> {
        let executor = stock_agent_principal(&realm, agent)?;
        Ok(Self {
            realm,
            agent: agent.to_string(),
            executor,
        })
    }

    /// An open composition's agent with `executor`, the executor that
    /// composition actually uses (its supplied principals'): the live read.
    pub(super) fn live(realm: RealmId, agent: &str, executor: PrincipalRef) -> Self {
        Self {
            realm,
            agent: agent.to_string(),
            executor,
        }
    }
}

/// Which selection an observation read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StockGrantObservationCoverage {
    /// The realm's persisted selection, which realms opened afterwards
    /// compose, read under the realm's administrative custody
    /// ([`observe_stock_agent_grants`]) or a staged realm's own custody
    /// (`StagedStockRealm::observe_stock_agent_grants`). It is not an open
    /// realm's retained selection.
    PersistedSelection,
    /// The selection an open realm's composition retained when it opened,
    /// which that realm's prompts use whatever the persisted selection says
    /// now, resolved through that composition's own grant owner
    /// ([`super::StockGovernance::observe_retained_grants`]).
    RetainedSelection,
}

/// Where a slot's selected lineage comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockSelectionSource {
    /// The slot's own per-target grant.
    PerTarget,
    /// An older selection's single worker grant, which serves every worker
    /// slot in that older mode only.
    LegacyWorker,
}

/// The raw operations (`mcp/<operation>` actions) an MCP grant's action
/// dimension names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockMcpOperations {
    /// The action dimension is unrestricted.
    All,
    /// Exactly these raw operations.
    Exact(BTreeSet<String>),
}

/// An MCP grant's action dimension read for its raw operations, with
/// everything else in it kept as it is. It claims neither the connector
/// approval nor any permission to invoke: both are decided at use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMcpOperationsView {
    pub operations: StockMcpOperations,
    /// The dimension's other exact actions, such as a stock grant's
    /// installation entitlement and connector approval.
    pub other_actions: BTreeSet<ActionRef>,
    /// The dimension's unresolved facts.
    pub unresolved: BTreeSet<UnresolvedConstraint>,
}

/// The lifetime of a resolved lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockGrantLifetime {
    /// No link of the lineage has a window.
    Unrestricted,
    /// Some link has one: the owner's earliest end across the lineage, which
    /// stays finite even when it is `u64::MAX`.
    Finite { expires_at_ms: u64 },
}

/// What the grant owner resolved for one lineage, copied from its
/// resolution. It is not a grant, not a permission and not the owner's
/// `ResolvedGrant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockResolvedGrantFacts {
    /// The leaf restrictions the owner resolved for the agent's executor.
    pub restrictions: ExecutionRestrictions,
    /// Each link's retained bounds, root to leaf.
    pub lineage_restrictions: Vec<ExecutionRestrictions>,
    pub lifetime: StockGrantLifetime,
    /// For an MCP slot, the raw operations of `restrictions`.
    pub mcp_operations: Option<StockMcpOperationsView>,
}

/// The grant owner's conclusion on one selected lineage when the
/// observation was made. `Current` is that moment's resolution, not a
/// permission until the lineage expires.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StockGrantConclusion {
    /// The owner resolved the exact lineage for the agent's executor.
    Current(StockResolvedGrantFacts),
    /// The owner refused the exact lineage for the agent's executor
    /// ([`GrantRefusal::Denied`]). It does not say why (revoked, expired,
    /// not yet valid or another executor), and this names no cause.
    NotCurrent,
    /// The owner could not conclude ([`GrantRefusal::Unavailable`]).
    Unavailable,
}

/// A slot's selected lineage, where it comes from, and the grant owner's
/// conclusion on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockSelectedLineage {
    pub lineage: GrantLineageRef,
    pub source: StockSelectionSource,
    pub conclusion: StockGrantConclusion,
}

/// One requested slot of an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockSlotObservation {
    pub slot: StockGrantSlot,
    /// The lineage the observed selection names for the slot; `None` when it
    /// names none, which says nothing about grants issued earlier.
    pub selected: Option<StockSelectedLineage>,
}

/// One observation of an agent's grant selection and of the grant owner's
/// conclusion on each selected lineage. It covers exactly the requested
/// slots of this agent's own selection: no other agent, no unrequested slot
/// and no child or inherited grant. It claims no credential or account
/// readiness, and it guarantees nothing to a later mutation or call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockGrantObservation {
    pub realm: RealmId,
    pub agent: String,
    /// The executor each selected lineage was resolved for.
    pub executor: PrincipalRef,
    pub coverage: StockGrantObservationCoverage,
    /// One entry per requested slot, in request order.
    pub slots: Vec<StockSlotObservation>,
}

/// The offline administrative read: `agent`'s selected grants in `slots` as
/// the realm's persisted selection names them, under the realm's
/// administrative custody with its restored grant owner (see
/// [`StockGrantObservationCoverage::PersistedSelection`]). While a host
/// serves the realm its custody is held and this read is refused; that host
/// observes its own retained selection
/// ([`super::StockGovernance::observe_retained_grants`]).
///
/// # Errors
/// Unavailable evidence for the whole observation, never a slot without a
/// selection: an invalid realm or agent, a realm that is not governed or
/// whose custody is held, a grant owner that does not restore, or a
/// selection that exists but cannot be read.
pub async fn observe_stock_agent_grants(
    realms_root: &Path,
    realm_id: &str,
    agent: &str,
    slots: &[StockGrantSlot],
) -> Result<StockGrantObservation, StockGovernanceError> {
    let realm = RealmId::parse(realm_id)
        .map_err(|error| StockGovernanceError::Principal(error.to_string()))?;
    let subject = ObservedAgent::new(realm, agent)?;
    let bundle = administrative_custody(realms_root, realm_id).await?;
    let observation = observe_persisted(realms_root, subject, slots).await;
    drop(bundle);
    observation
}

/// [`observe_stock_agent_grants`] under a realm custody the caller already
/// holds: a staged realm's own.
pub(crate) async fn observe_persisted_under_custody(
    realms_root: &Path,
    realm_id: &str,
    agent: &str,
    slots: &[StockGrantSlot],
) -> Result<StockGrantObservation, StockGovernanceError> {
    let realm = RealmId::parse(realm_id)
        .map_err(|error| StockGovernanceError::Principal(error.to_string()))?;
    let subject = ObservedAgent::new(realm, agent)?;
    observe_persisted(realms_root, subject, slots).await
}

/// The persisted selection, through the realm's grant owner restored for
/// this read only, under the realm custody the caller holds.
async fn observe_persisted(
    realms_root: &Path,
    subject: ObservedAgent,
    slots: &[StockGrantSlot],
) -> Result<StockGrantObservation, StockGovernanceError> {
    let owner = restore_owner(realms_root, subject.realm.as_str())?;
    let paths = meerkat_store::realm_paths_in(realms_root, subject.realm.as_str());
    let file = super::read_stock_grant_file(&paths).await?;
    let refs = file
        .as_ref()
        .and_then(|file| file.agents.get(&subject.agent));
    Ok(observe_selection(
        &owner,
        subject,
        refs,
        slots,
        StockGrantObservationCoverage::PersistedSelection,
    ))
}

/// Observe `refs`, the selection `coverage` names for the subject's agent
/// (`None` when it records nothing for that agent), through `owner`.
pub(super) fn observe_selection(
    owner: &LocalGrantAuthority,
    subject: ObservedAgent,
    refs: Option<&AgentGrantRefs>,
    slots: &[StockGrantSlot],
    coverage: StockGrantObservationCoverage,
) -> StockGrantObservation {
    let slots = slots
        .iter()
        .map(|slot| StockSlotObservation {
            slot: slot.clone(),
            selected: refs
                .and_then(|refs| refs.selected_for(slot))
                .map(|(lineage, source)| StockSelectedLineage {
                    lineage: lineage.clone(),
                    source,
                    conclusion: conclude(owner, slot, lineage, &subject.executor),
                }),
        })
        .collect();
    StockGrantObservation {
        realm: subject.realm,
        agent: subject.agent,
        executor: subject.executor,
        coverage,
        slots,
    }
}

/// The grant owner's conclusion on `lineage` for `executor`: the controller
/// slot as native controller admission resolves it, every other slot as an
/// operation's lineage.
fn conclude(
    owner: &LocalGrantAuthority,
    slot: &StockGrantSlot,
    lineage: &GrantLineageRef,
    executor: &PrincipalRef,
) -> StockGrantConclusion {
    let references = std::slice::from_ref(lineage);
    let resolved = if matches!(slot, StockGrantSlot::Controller) {
        owner.resolve_controller_lineage(references, executor, None)
    } else {
        owner.resolve_lineage(references, executor, None)
    };
    match resolved {
        Ok(resolved) => {
            let lineage_restrictions = resolved.lineage_restrictions().to_vec();
            let lifetime = if lineage_restrictions
                .iter()
                .all(|link| matches!(link.lifetime.bound(), LifetimeBound::Unrestricted))
            {
                StockGrantLifetime::Unrestricted
            } else {
                StockGrantLifetime::Finite {
                    expires_at_ms: resolved.expires_at_ms(),
                }
            };
            let restrictions = resolved.restrictions().clone();
            let mcp_operations = matches!(
                slot,
                StockGrantSlot::Mcp(_) | StockGrantSlot::CheckedMcpRoute { .. }
            )
            .then(|| mcp_operations_view(&restrictions));
            StockGrantConclusion::Current(StockResolvedGrantFacts {
                restrictions,
                lineage_restrictions,
                lifetime,
                mcp_operations,
            })
        }
        Err(GrantRefusal::Denied) => StockGrantConclusion::NotCurrent,
        Err(
            GrantRefusal::Unavailable
            | GrantRefusal::Duplicate
            | GrantRefusal::ControllerInUse
            | GrantRefusal::AdmissionFence,
        ) => StockGrantConclusion::Unavailable,
    }
}

/// `restrictions`' action dimension read for MCP raw operations
/// (`mcp/<operation>` actions), keeping every other exact action and every
/// unresolved fact as it is.
fn mcp_operations_view(restrictions: &ExecutionRestrictions) -> StockMcpOperationsView {
    let (operations, other_actions) = match restrictions.actions.bound() {
        SetBound::Unrestricted => (StockMcpOperations::All, BTreeSet::new()),
        SetBound::Exact(actions) => (
            StockMcpOperations::Exact(
                actions
                    .iter()
                    .filter(|action| action.feature == MCP_FEATURE)
                    .map(|action| action.action.clone())
                    .collect(),
            ),
            actions
                .iter()
                .filter(|action| action.feature != MCP_FEATURE)
                .cloned()
                .collect(),
        ),
    };
    StockMcpOperationsView {
        operations,
        other_actions,
        unresolved: restrictions.actions.unresolved_facts().clone(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests;
