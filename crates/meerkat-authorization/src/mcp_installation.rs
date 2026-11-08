//! Whether a grant's restrictions cover one realm MCP installation.
//!
//! A pure bound check over already resolved grant facts. It does not resolve
//! a grant, read current authority, identify the agent, or decide an
//! attachment: the owner that decides attachment resolves the exact lineage
//! first and conjoins this with its own installation and policy checks.

use meerkat_authorization_contracts::constraints::{
    ActionRef, BoundMatch, DomainBound, ExecutionRestrictions, ResourceDomain,
    RestrictionDimension, RestrictionFailure, SetBound,
};
use meerkat_core::McpConnectorApproval;
use meerkat_core::auth::PrincipalRef;

use crate::grants::ResolvedGrant;

/// The action feature that names a realm MCP installation in a grant.
pub const MCP_INSTALLATION_FEATURE: &str = "mcp_installation";

/// The action feature that records the connector binding a grant approved
/// for one server: `<server>/<token>`, where the token is
/// [`McpConnectorApproval::token`] (`none` for an explicit no-connector
/// approval).
pub const MCP_CONNECTOR_FEATURE: &str = "mcp_connector";

/// The namespace prefix of the resource domain that records a connector
/// approval: `mcp-connector/<server>/<token>`.
pub const MCP_CONNECTOR_NAMESPACE_PREFIX: &str = "mcp-connector/";

/// The action naming `approval` for `server`. The token never contains `/`,
/// so the last `/` separates it from the server.
pub fn mcp_connector_action(server: &str, approval: &McpConnectorApproval) -> ActionRef {
    ActionRef {
        feature: MCP_CONNECTOR_FEATURE.to_owned(),
        action: format!("{server}/{}", approval.token()),
    }
}

/// The resource domain naming `approval` for `server`, owned by `authority`
/// (the operator that owns the server's installation domain).
pub fn mcp_connector_domain(
    authority: &PrincipalRef,
    server: &str,
    approval: &McpConnectorApproval,
) -> ResourceDomain {
    connector_domain_of(authority, &mcp_connector_action(server, approval))
}

fn connector_domain_of(authority: &PrincipalRef, connector: &ActionRef) -> ResourceDomain {
    ResourceDomain {
        authority: authority.clone(),
        namespace: format!("{MCP_CONNECTOR_NAMESPACE_PREFIX}{}", connector.action),
    }
}

/// What a grant's restrictions record for one server's connector approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectorBound {
    /// The grant explicitly lists this exact approval.
    Approved,
    /// The grant lists no connector approval for this server: a grant
    /// issued before approvals were recorded, or one whose bounds are
    /// wildcards. Never read as `none`, and never filled from current
    /// configuration.
    Unrecorded,
    /// The grant lists only another approval for this server.
    Other,
}

/// One connector approval a grant names: the authority owning the domain
/// form (`None` for the action form, which names no authority and so
/// applies to the server under every authority), the server and the token.
type NamedApproval<'a> = (Option<&'a PrincipalRef>, &'a str, &'a str);

/// Every connector approval `link` names in an exact bound, in either form;
/// `None` for one that is malformed. With `resolved_only`, a bound carrying
/// unresolved facts names nothing.
fn named_approvals(
    link: &ExecutionRestrictions,
    resolved_only: bool,
) -> impl Iterator<Item = Option<NamedApproval<'_>>> {
    let actions = match link.actions.bound() {
        SetBound::Exact(actions)
            if !resolved_only || link.actions.unresolved_facts().is_empty() =>
        {
            Some(actions)
        }
        _ => None,
    };
    // Only exact domains name approvals: an owner-scoped or unrestricted
    // domain bound names none, so its lineage reads as `Unrecorded`.
    let domains = match link.resource_domains.bound() {
        DomainBound::Exact(domains)
            if !resolved_only || link.resource_domains.unresolved_facts().is_empty() =>
        {
            Some(domains)
        }
        _ => None,
    };
    let from_actions = actions
        .into_iter()
        .flatten()
        .filter(|action| action.feature == MCP_CONNECTOR_FEATURE)
        .map(|action| {
            action
                .action
                .rsplit_once('/')
                .map(|(server, token)| (None, server, token))
        });
    let from_domains = domains.into_iter().flatten().filter_map(|domain| {
        domain
            .namespace
            .strip_prefix(MCP_CONNECTOR_NAMESPACE_PREFIX)
            .map(|named| {
                named
                    .rsplit_once('/')
                    .map(|(server, token)| (Some(&domain.authority), server, token))
            })
    });
    from_actions.chain(from_domains)
}

/// Whether one named approval and another name the same installation: the
/// same server under the same authority, or either in the unqualified
/// action form. Equal server names under different authorities are distinct
/// installations and never compared.
fn same_installation(left: &NamedApproval<'_>, right: &NamedApproval<'_>) -> bool {
    left.1 == right.1 && (left.0.is_none() || right.0.is_none() || left.0 == right.0)
}

/// Every approval `link` names (see [`named_approvals`]), with each
/// action-form approval qualified by the link's own domain-form approvals
/// for that server: one authority qualifies it; none leaves it unqualified
/// (it then compares with every authority); two or more make the link
/// ambiguous, so this returns `None`, as for a malformed approval.
fn qualified_approvals(link: &ExecutionRestrictions) -> Option<Vec<NamedApproval<'_>>> {
    let named = named_approvals(link, false).collect::<Option<Vec<_>>>()?;
    named
        .iter()
        .map(|approval| match approval {
            (None, server, token) => {
                let mut owners = named
                    .iter()
                    .filter(|other| other.1 == *server)
                    .filter_map(|other| other.0);
                match owners.next() {
                    None => Some((None, *server, *token)),
                    Some(first) if owners.all(|other| other == first) => {
                        Some((Some(first), *server, *token))
                    }
                    Some(_) => None,
                }
            }
            qualified => Some(*qualified),
        })
        .collect()
}

/// Whether every installation's connector approval is the same wherever a
/// lineage names one, in either form (keyed by domain authority and
/// server; an action-form approval by the installation its own link
/// identifies). A grant owner refuses to issue a grant that would name a
/// second approval: conjoining the two forms independently would otherwise
/// let a child replace the approval its parent recorded.
pub fn lineage_approvals_agree<'a>(
    links: impl IntoIterator<Item = &'a ExecutionRestrictions>,
) -> bool {
    let mut named: Vec<NamedApproval<'a>> = Vec::new();
    for link in links {
        let Some(approvals) = qualified_approvals(link) else {
            return false;
        };
        for approval in approvals {
            if named
                .iter()
                .any(|other| same_installation(other, &approval) && other.2 != approval.2)
            {
                return false;
            }
            named.push(approval);
        }
    }
    true
}

/// Whether every connector approval `child` names is one `ancestor` (the
/// lineage's operator-issued root, or the child's actual parent) records for
/// the same installation, each qualified by its own link. A child is issued
/// by its parent's grantee, so it may keep or narrow away an approval its
/// parent still holds, never originate one, switch to another, or regain
/// one an intermediate grant removed.
pub fn child_approvals_from(
    ancestor: &ExecutionRestrictions,
    child: &ExecutionRestrictions,
) -> bool {
    let (Some(recorded), Some(named)) = (qualified_approvals(ancestor), qualified_approvals(child))
    else {
        return false;
    };
    named.iter().all(|approval| {
        recorded
            .iter()
            .any(|root| same_installation(root, approval) && root.2 == approval.2)
    })
}

/// Whether an unqualified (action-form) approval in `link` binds to the
/// installation `installation` of `server`. It binds only when the link
/// identifies that one installation: its domain-form approvals for the
/// server name exactly that authority, or, naming none, its exact resource
/// domains hold that installation domain and no other authority's domain of
/// the same name. Otherwise the action form is ambiguous and binds nothing.
fn action_binds(link: &ExecutionRestrictions, server: &str, installation: &ResourceDomain) -> bool {
    // An owner-scoped or unrestricted domain bound identifies no one
    // installation, so the action form is ambiguous there.
    let DomainBound::Exact(domains) = link.resource_domains.bound() else {
        return false;
    };
    let mut approving = domains.iter().filter_map(|domain| {
        domain
            .namespace
            .strip_prefix(MCP_CONNECTOR_NAMESPACE_PREFIX)
            .and_then(|named| named.rsplit_once('/'))
            .filter(|(named, _)| *named == server)
            .map(|_| &domain.authority)
    });
    match approving.next() {
        Some(first) => {
            first == &installation.authority
                && approving.all(|authority| authority == &installation.authority)
        }
        None => {
            domains.contains(installation)
                && !domains.iter().any(|domain| {
                    domain.namespace == installation.namespace
                        && domain.authority != installation.authority
                })
        }
    }
}

/// Classify the approval `connector` (an [`mcp_connector_action`]) of the
/// installation `installation` for one set of `restrictions`. See
/// [`lineage_connector_bound`].
pub fn connector_bound(
    restrictions: &ExecutionRestrictions,
    connector: &ActionRef,
    installation: &ResourceDomain,
) -> McpConnectorBound {
    lineage_connector_bound(&[], restrictions, connector, installation)
}

/// Classify the approval `connector` of the installation `installation` (its
/// domain, whose authority owns the connector domains) for a resolved grant:
/// its effective `leaf` bounds and each link's retained bounds, root first.
///
/// Every link must still record the approval, from the operator-issued root
/// through each intermediate grant: a link that removed it ends it for every
/// descendant. An approval is recorded only by explicit membership: its
/// [`mcp_connector_domain`] (under the installation's authority) in an exact
/// resource domain set, or the action in an exact action set where the link
/// identifies this installation (its domain-form approvals for the server
/// name exactly that authority or, naming none, its exact resource domains
/// hold the installation's domain and no other authority's domain of that
/// name). A wildcard bound never approves a connector, so a grant for every
/// raw operation of a server (unrestricted actions) still approves exactly
/// the one connector its domains name. Any link (or the leaf) naming another
/// approval for this installation makes it [`McpConnectorBound::Other`];
/// another authority's installation of the same server name is not compared.
pub fn lineage_connector_bound(
    links: &[ExecutionRestrictions],
    leaf: &ExecutionRestrictions,
    connector: &ActionRef,
    installation: &ResourceDomain,
) -> McpConnectorBound {
    let Some((server, expected)) = connector.action.rsplit_once('/') else {
        return McpConnectorBound::Unrecorded;
    };
    let applies =
        |link: &ExecutionRestrictions, approval: &Option<NamedApproval<'_>>| match approval {
            Some((Some(owner), named, _)) => *named == server && *owner == &installation.authority,
            Some((None, named, _)) => *named == server && action_binds(link, server, installation),
            None => true,
        };
    let conflicting = links.iter().chain(std::iter::once(leaf)).any(|link| {
        named_approvals(link, false)
            .filter(|approval| applies(link, approval))
            .any(|approval| approval.is_none_or(|(_, _, token)| token != expected))
    });
    if conflicting {
        return McpConnectorBound::Other;
    }
    let records = |link: &ExecutionRestrictions| {
        named_approvals(link, true)
            .filter(|approval| applies(link, approval))
            .any(|approval| approval.is_some_and(|(_, _, token)| token == expected))
    };
    // A missing approval in any link (root included) stays missing evidence,
    // whatever a later link names.
    if links.iter().chain(std::iter::once(leaf)).all(records) {
        McpConnectorBound::Approved
    } else {
        McpConnectorBound::Unrecorded
    }
}

/// One installed MCP server of one realm, as a grant names it: the realm's
/// installation domain, the server name and the connector approval of the
/// descriptor being installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInstallationRef {
    action: ActionRef,
    connector: ActionRef,
    domain: ResourceDomain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an MCP installation needs a server name")]
pub struct EmptyServerName;

impl McpInstallationRef {
    /// `domain` is the realm's installation domain, owned by the realm's
    /// configuration authority; equal server names in different realms are
    /// different installations.
    /// `connector` is the approval of the descriptor actually being
    /// installed, never one recovered from the grant.
    pub fn new(
        domain: ResourceDomain,
        server: &str,
        connector: &McpConnectorApproval,
    ) -> Result<Self, EmptyServerName> {
        if server.trim().is_empty() {
            return Err(EmptyServerName);
        }
        Ok(Self {
            action: ActionRef {
                feature: MCP_INSTALLATION_FEATURE.to_owned(),
                action: server.to_owned(),
            },
            connector: mcp_connector_action(server, connector),
            domain,
        })
    }

    /// The exact action a grant lists to cover this installation.
    pub fn action(&self) -> &ActionRef {
        &self.action
    }

    /// The action naming this installation's connector approval.
    pub fn connector(&self) -> &ActionRef {
        &self.connector
    }

    pub fn domain(&self) -> &ResourceDomain {
        &self.domain
    }
}

/// Whether `restrictions` entitle the grantee to `installation` at `now_ms`.
///
/// The action bound must contain the installation, the grant must explicitly
/// list its exact connector approval ([`connector_bound`]), the
/// resource-domain bound must contain the installation's domain, the
/// lifetime must be current, and the delegation depth must have no
/// unresolved facts. This entitlement predicate has no destination or
/// processor to select, so it does not match those bounds; any unresolved
/// fact in them still refuses. It is never the complete attachment
/// decision: the actual attachment publishes to a real destination and
/// hydrates a real source, and the existing policy owner must still apply
/// the audience, processor, account and operation restrictions to those.
/// Success is a necessary condition only; it is not a permit.
///
/// # Errors
/// The first failing dimension, in the order action, resource domain,
/// processor, audience, lifetime, delegation depth.
pub fn check_installation_bounds(
    restrictions: &ExecutionRestrictions,
    installation: &McpInstallationRef,
    now_ms: u64,
) -> Result<(), RestrictionFailure> {
    let checks = [
        (
            RestrictionDimension::Action,
            restrictions.actions.check(&installation.action),
        ),
        (
            RestrictionDimension::ResourceDomain,
            restrictions.resource_domains.check(&installation.domain),
        ),
        // After the domain: the action form binds only to an installation
        // the grant's domains hold.
        (
            RestrictionDimension::Action,
            match connector_bound(restrictions, &installation.connector, &installation.domain) {
                McpConnectorBound::Approved => BoundMatch::Matches,
                McpConnectorBound::Unrecorded | McpConnectorBound::Other => {
                    BoundMatch::OutsideBounds
                }
            },
        ),
        (
            RestrictionDimension::Processor,
            unresolved_facts(restrictions.processors.unresolved_facts()),
        ),
        (
            RestrictionDimension::Audience,
            unresolved_facts(restrictions.audiences.unresolved_facts()),
        ),
        (
            RestrictionDimension::Lifetime,
            restrictions.lifetime.check(now_ms),
        ),
        (
            RestrictionDimension::DelegationDepth,
            restrictions.delegation_depth.check_current_use(),
        ),
    ];
    for (dimension, check) in checks {
        match check {
            BoundMatch::Matches => {}
            BoundMatch::OutsideBounds => {
                return Err(RestrictionFailure::OutsideBounds { dimension });
            }
            BoundMatch::Unresolved(facts) => {
                return Err(RestrictionFailure::Unresolved { dimension, facts });
            }
        }
    }
    Ok(())
}

/// [`check_installation_bounds`] over a freshly resolved grant's effective
/// leaf restrictions, also refusing once the resolved lineage's earliest
/// expiry has passed. Publication currency stays the caller's final check.
///
/// # Errors
/// As [`check_installation_bounds`]; an expired lineage is outside the
/// lifetime bound.
pub fn check_resolved_installation(
    grant: &ResolvedGrant,
    installation: &McpInstallationRef,
    now_ms: u64,
) -> Result<(), RestrictionFailure> {
    if now_ms >= grant.expires_at_ms() {
        return Err(RestrictionFailure::OutsideBounds {
            dimension: RestrictionDimension::Lifetime,
        });
    }
    check_installation_bounds(grant.restrictions(), installation, now_ms)?;
    // The approval must also be the only one its lineage names.
    match lineage_connector_bound(
        grant.lineage_restrictions(),
        grant.restrictions(),
        &installation.connector,
        &installation.domain,
    ) {
        McpConnectorBound::Approved => Ok(()),
        McpConnectorBound::Unrecorded | McpConnectorBound::Other => {
            Err(RestrictionFailure::OutsideBounds {
                dimension: RestrictionDimension::Action,
            })
        }
    }
}

fn unresolved_facts(
    facts: &std::collections::BTreeSet<
        meerkat_authorization_contracts::constraints::UnresolvedConstraint,
    >,
) -> BoundMatch {
    if facts.is_empty() {
        BoundMatch::Matches
    } else {
        BoundMatch::Unresolved(facts.clone())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_authorization_contracts::constraints::{
        AudienceRef, DelegationDepth, DomainRestriction, ExactRestriction, LifetimeRestriction,
        ProcessorRef, UnresolvedConstraint,
    };
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    fn who(name: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            name,
            TrustDomainId::new("installation").expect("domain"),
        )
        .expect("principal")
    }
    fn realm(name: &str) -> ResourceDomain {
        ResourceDomain {
            authority: who("realm-config"),
            namespace: format!("realm/{name}/mcp"),
        }
    }
    fn sentinel() -> McpInstallationRef {
        McpInstallationRef::new(
            realm("alpha"),
            "sentinel",
            &McpConnectorApproval::NoConnector,
        )
        .expect("installation")
    }
    /// A grant naming each server with an explicit no-connector approval.
    fn naming(servers: &[&str]) -> ExecutionRestrictions {
        ExecutionRestrictions {
            actions: ExactRestriction::exact(servers.iter().flat_map(|server| {
                [
                    ActionRef {
                        feature: MCP_INSTALLATION_FEATURE.to_owned(),
                        action: (*server).to_owned(),
                    },
                    mcp_connector_action(server, &McpConnectorApproval::NoConnector),
                ]
            })),
            resource_domains: DomainRestriction::exact([realm("alpha")]),
            ..ExecutionRestrictions::unrestricted()
        }
    }
    fn outside(dimension: RestrictionDimension) -> Result<(), RestrictionFailure> {
        Err(RestrictionFailure::OutsideBounds { dimension })
    }

    #[test]
    fn a_grant_naming_the_installation_covers_it() {
        assert_eq!(
            check_installation_bounds(&naming(&["sentinel"]), &sentinel(), 10),
            Ok(())
        );
        assert_eq!(
            check_installation_bounds(&naming(&["other", "sentinel"]), &sentinel(), 10),
            Ok(())
        );
        assert_eq!(
            check_installation_bounds(&ExecutionRestrictions::unrestricted(), &sentinel(), 10),
            outside(RestrictionDimension::Action),
            "an unrestricted grant lists no connector approval, so it covers no installation"
        );
    }

    #[test]
    fn another_server_realm_or_feature_is_outside() {
        assert_eq!(
            check_installation_bounds(&naming(&["other"]), &sentinel(), 10),
            outside(RestrictionDimension::Action)
        );
        let beta = McpInstallationRef::new(
            realm("beta"),
            "sentinel",
            &McpConnectorApproval::NoConnector,
        )
        .expect("installation");
        assert_eq!(
            check_installation_bounds(&naming(&["sentinel"]), &beta, 10),
            outside(RestrictionDimension::ResourceDomain),
            "the same server name in another realm is another installation"
        );
        let tool_action = ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "mcp".to_owned(),
                action: "sentinel".to_owned(),
            }]),
            ..ExecutionRestrictions::unrestricted()
        };
        assert_eq!(
            check_installation_bounds(&tool_action, &sentinel(), 10),
            outside(RestrictionDimension::Action)
        );
        assert_eq!(
            McpInstallationRef::new(realm("alpha"), "  ", &McpConnectorApproval::NoConnector),
            Err(EmptyServerName)
        );
    }

    /// The grant covers an installation only with the exact connector
    /// approval it recorded: another account's binding, a connector added to
    /// a no-connector server, or a grant that recorded no approval at all is
    /// outside, and the three are told apart.
    #[test]
    fn only_the_recorded_connector_approval_is_covered() {
        let account_a = McpConnectorApproval::Connector("sha256:aa".into());
        let account_b = McpConnectorApproval::Connector("sha256:bb".into());
        let approving = |approval: &McpConnectorApproval| ExecutionRestrictions {
            actions: ExactRestriction::exact([
                ActionRef {
                    feature: MCP_INSTALLATION_FEATURE.to_owned(),
                    action: "sentinel".to_owned(),
                },
                mcp_connector_action("sentinel", approval),
            ]),
            resource_domains: DomainRestriction::exact([realm("alpha")]),
            ..ExecutionRestrictions::unrestricted()
        };
        let installing = |approval: &McpConnectorApproval| {
            McpInstallationRef::new(realm("alpha"), "sentinel", approval).expect("installation")
        };
        let for_a = approving(&account_a);
        assert_eq!(
            check_installation_bounds(&for_a, &installing(&account_a), 10),
            Ok(())
        );
        assert_eq!(
            connector_bound(&for_a, installing(&account_a).connector(), &realm("alpha")),
            McpConnectorBound::Approved
        );
        assert_eq!(
            check_installation_bounds(&for_a, &installing(&account_b), 10),
            outside(RestrictionDimension::Action),
            "a same-name server bound to another account is not covered"
        );
        assert_eq!(
            connector_bound(&for_a, installing(&account_b).connector(), &realm("alpha")),
            McpConnectorBound::Other
        );
        let no_connector = approving(&McpConnectorApproval::NoConnector);
        assert_eq!(
            check_installation_bounds(&no_connector, &installing(&account_a), 10),
            outside(RestrictionDimension::Action),
            "a connector added to a no-connector server is not covered"
        );
        let legacy = ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: MCP_INSTALLATION_FEATURE.to_owned(),
                action: "sentinel".to_owned(),
            }]),
            resource_domains: DomainRestriction::exact([realm("alpha")]),
            ..ExecutionRestrictions::unrestricted()
        };
        for approval in [&McpConnectorApproval::NoConnector, &account_a] {
            assert_eq!(
                check_installation_bounds(&legacy, &installing(approval), 10),
                outside(RestrictionDimension::Action),
                "a grant that recorded no approval covers no connector, not even none"
            );
            assert_eq!(
                connector_bound(&legacy, installing(approval).connector(), &realm("alpha")),
                McpConnectorBound::Unrecorded
            );
        }
        // Another server's approval is not this server's record.
        assert_eq!(
            connector_bound(
                &approving(&account_a),
                &mcp_connector_action("sentinel-2", &account_a),
                &realm("alpha")
            ),
            McpConnectorBound::Unrecorded
        );
    }

    /// A grant for every raw operation (unrestricted actions) approves no
    /// connector through that wildcard: it approves exactly the one its
    /// resource domains list, and none when they list none.
    #[test]
    fn a_wildcard_action_bound_approves_no_connector() {
        let account_a = McpConnectorApproval::Connector("sha256:aa".into());
        let account_b = McpConnectorApproval::Connector("sha256:bb".into());
        let installing = |approval: &McpConnectorApproval| {
            McpInstallationRef::new(realm("alpha"), "sentinel", approval).expect("installation")
        };
        let every_operation = |domains: Vec<ResourceDomain>| ExecutionRestrictions {
            resource_domains: DomainRestriction::exact(domains),
            ..ExecutionRestrictions::unrestricted()
        };
        let unapproved = every_operation(vec![realm("alpha")]);
        for approval in [&account_a, &account_b, &McpConnectorApproval::NoConnector] {
            assert_eq!(
                check_installation_bounds(&unapproved, &installing(approval), 10),
                outside(RestrictionDimension::Action),
                "the wildcard alone approves no connector"
            );
            assert_eq!(
                connector_bound(
                    &unapproved,
                    installing(approval).connector(),
                    &realm("alpha")
                ),
                McpConnectorBound::Unrecorded
            );
        }
        let approved_a = every_operation(vec![
            realm("alpha"),
            mcp_connector_domain(&who("realm-config"), "sentinel", &account_a),
        ]);
        assert_eq!(
            check_installation_bounds(&approved_a, &installing(&account_a), 10),
            Ok(())
        );
        for other in [&account_b, &McpConnectorApproval::NoConnector] {
            assert_eq!(
                check_installation_bounds(&approved_a, &installing(other), 10),
                outside(RestrictionDimension::Action)
            );
            assert_eq!(
                connector_bound(&approved_a, installing(other).connector(), &realm("alpha")),
                McpConnectorBound::Other
            );
        }
        // Another authority's domain is not the operator's approval.
        let foreign = every_operation(vec![
            realm("alpha"),
            mcp_connector_domain(&who("intruder"), "sentinel", &account_a),
        ]);
        assert_eq!(
            check_installation_bounds(&foreign, &installing(&account_a), 10),
            outside(RestrictionDimension::Action)
        );
    }

    #[test]
    fn lifetime_and_unresolved_facts_refuse() {
        let windowed = ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(5, 20),
            ..naming(&["sentinel"])
        };
        assert_eq!(
            check_installation_bounds(&windowed, &sentinel(), 10),
            Ok(())
        );
        assert_eq!(
            check_installation_bounds(&windowed, &sentinel(), 20),
            outside(RestrictionDimension::Lifetime)
        );
        assert_eq!(
            check_installation_bounds(&windowed, &sentinel(), 1),
            outside(RestrictionDimension::Lifetime)
        );
        for (restrictions, dimension) in [
            (
                ExecutionRestrictions {
                    actions: ExactRestriction::unresolved(UnresolvedConstraint::Unknown),
                    ..ExecutionRestrictions::unrestricted()
                },
                RestrictionDimension::Action,
            ),
            (
                ExecutionRestrictions {
                    processors: ExactRestriction::unresolved(UnresolvedConstraint::Unavailable),
                    ..naming(&["sentinel"])
                },
                RestrictionDimension::Processor,
            ),
            (
                ExecutionRestrictions {
                    audiences: ExactRestriction::unresolved(UnresolvedConstraint::Absent),
                    ..naming(&["sentinel"])
                },
                RestrictionDimension::Audience,
            ),
            (
                ExecutionRestrictions {
                    delegation_depth: DelegationDepth::unresolved(UnresolvedConstraint::Unknown),
                    ..naming(&["sentinel"])
                },
                RestrictionDimension::DelegationDepth,
            ),
        ] {
            assert!(
                matches!(
                    check_installation_bounds(&restrictions, &sentinel(), 10),
                    Err(RestrictionFailure::Unresolved { dimension: got, .. }) if got == dimension
                ),
                "{dimension:?}"
            );
        }
    }

    /// This predicate selects no destination or processor; their bounds are
    /// left to the attachment's policy owner.
    #[test]
    fn processor_and_audience_bounds_are_not_selected_by_this_predicate() {
        let narrowed = ExecutionRestrictions {
            processors: ExactRestriction::exact([ProcessorRef::Principal {
                principal: who("model"),
            }]),
            audiences: ExactRestriction::exact([AudienceRef::Principal {
                principal: who("reader"),
            }]),
            ..naming(&["sentinel"])
        };
        assert_eq!(
            check_installation_bounds(&narrowed, &sentinel(), 10),
            Ok(())
        );
    }

    #[test]
    fn a_resolved_grant_is_checked_through_its_leaf_and_lineage_expiry() {
        use crate::clock::{LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError};
        use crate::grants::{
            IsolatedGrantTestCustody, LocalGrantAuthority, LocalGrantConfiguration,
        };
        use crate::publication::LocalAuthorizationPublication;
        use meerkat_authorization_contracts::evidence::EvidenceId;

        struct At(u64);
        impl LocalAuthorizationClock for At {
            fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
                Ok(LocalAuthorizationTime {
                    unix_ms: self.0,
                    monotonic: meerkat_core::time_compat::Instant::now(),
                })
            }
        }
        let owner = LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            std::sync::Arc::new(At(10)),
        )
        .expect("owner");
        let grant = owner
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &who("operator"),
                EvidenceId::new("agent-a-sentinel").expect("id"),
                who("agent-a"),
                None,
                ExecutionRestrictions {
                    lifetime: LifetimeRestriction::window(1, 50),
                    ..naming(&["sentinel"])
                },
            )
            .expect("issue");
        let resolved = owner
            .resolve_lineage(std::slice::from_ref(&grant), &who("agent-a"), None)
            .expect("resolve for the grantee");
        assert_eq!(
            check_resolved_installation(&resolved, &sentinel(), 10),
            Ok(())
        );
        assert_eq!(
            check_resolved_installation(&resolved, &sentinel(), 50),
            outside(RestrictionDimension::Lifetime)
        );
        let other =
            McpInstallationRef::new(realm("alpha"), "other", &McpConnectorApproval::NoConnector)
                .expect("installation");
        assert_eq!(
            check_resolved_installation(&resolved, &other, 10),
            outside(RestrictionDimension::Action)
        );
        // Another agent cannot resolve the grant at all; revocation removes it.
        assert!(
            owner
                .resolve_lineage(std::slice::from_ref(&grant), &who("agent-b"), None)
                .is_err()
        );
        owner
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &who("operator"),
                &grant,
                &mut IsolatedGrantTestCustody,
            )
            .expect("revoke");
        assert!(
            owner
                .resolve_lineage(std::slice::from_ref(&grant), &who("agent-a"), None)
                .is_err()
        );
    }

    /// A delegated child cannot replace the connector approval its parent
    /// recorded through the other form (the two forms conjoin
    /// independently): the grant owner refuses to issue it, in either
    /// direction and even when the child narrows the parent's record away.
    /// A child naming the same approval resolves and covers it alone.
    #[test]
    fn a_child_cannot_replace_its_lineages_connector_approval() {
        use crate::clock::{LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError};
        use crate::grants::{GrantRefusal, LocalGrantAuthority, LocalGrantConfiguration};
        use crate::publication::LocalAuthorizationPublication;
        use meerkat_authorization_contracts::evidence::EvidenceId;

        struct At(u64);
        impl LocalAuthorizationClock for At {
            fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
                Ok(LocalAuthorizationTime {
                    unix_ms: self.0,
                    monotonic: meerkat_core::time_compat::Instant::now(),
                })
            }
        }
        let owner = LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            std::sync::Arc::new(At(10)),
        )
        .expect("owner");
        let account_a = McpConnectorApproval::Connector("sha256:aa".into());
        let account_b = McpConnectorApproval::Connector("sha256:bb".into());
        let installation = ActionRef {
            feature: MCP_INSTALLATION_FEATURE.to_owned(),
            action: "sentinel".to_owned(),
        };
        let by_domain = |approval: &McpConnectorApproval| ExecutionRestrictions {
            resource_domains: DomainRestriction::exact([
                realm("alpha"),
                mcp_connector_domain(&who("realm-config"), "sentinel", approval),
            ]),
            ..ExecutionRestrictions::unrestricted()
        };
        // The action form names the one installation domain it binds to.
        let by_action = |approval: &McpConnectorApproval| ExecutionRestrictions {
            actions: ExactRestriction::exact([
                installation.clone(),
                mcp_connector_action("sentinel", approval),
            ]),
            resource_domains: DomainRestriction::exact([realm("alpha")]),
            ..ExecutionRestrictions::unrestricted()
        };
        let issue_root = |id: &str, restrictions| {
            owner
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &who("operator"),
                    EvidenceId::new(id).expect("id"),
                    who("agent-a"),
                    None,
                    restrictions,
                )
                .expect("root")
        };
        let issue_child = |id: &str, parent, restrictions| {
            owner.issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &who("agent-a"),
                parent,
                EvidenceId::new(id).expect("id"),
                who("agent-b"),
                restrictions,
            )
        };

        let domain_a = issue_root("root-domain-a", by_domain(&account_a));
        assert_eq!(
            issue_child("child-action-b", &domain_a, by_action(&account_b)).err(),
            Some(GrantRefusal::Denied),
            "parent A by domain, child B by action"
        );
        let narrowed = ExecutionRestrictions {
            resource_domains: DomainRestriction::exact([realm("alpha")]),
            ..by_action(&account_b)
        };
        assert_eq!(
            issue_child("child-narrowed-b", &domain_a, narrowed).err(),
            Some(GrantRefusal::Denied),
            "dropping the parent's record does not free the child to name another"
        );
        // A root naming A by action under every domain, so a child's
        // domain-form request survives the conjunction.
        let action_a = issue_root(
            "root-action-a",
            ExecutionRestrictions {
                resource_domains: DomainRestriction::unrestricted(),
                ..by_action(&account_a)
            },
        );
        assert_eq!(
            issue_child("child-domain-b", &action_a, by_domain(&account_b)).err(),
            Some(GrantRefusal::Denied),
            "parent A by action, child B by domain"
        );
        assert_eq!(
            owner
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &who("operator"),
                    EvidenceId::new("root-two").expect("id"),
                    who("agent-a"),
                    None,
                    ExecutionRestrictions {
                        actions: by_action(&account_a).actions,
                        ..by_domain(&account_b)
                    },
                )
                .err(),
            Some(GrantRefusal::Denied),
            "one grant cannot name two approvals for a server"
        );

        // A child cannot originate an approval its root did not record: an
        // unrestricted (or legacy) root's grantee names none for a child.
        let unapproved = issue_root("root-unapproved", ExecutionRestrictions::unrestricted());
        for (label, requested) in [
            ("child-mint-action-a", by_action(&account_a)),
            ("child-mint-domain-a", by_domain(&account_a)),
        ] {
            assert_eq!(
                issue_child(label, &unapproved, requested).err(),
                Some(GrantRefusal::Denied),
                "{label}"
            );
        }

        // An intermediate grant that removed the approval ends it for its
        // descendants: its grantee cannot restore it through the other form.
        let removed = owner
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &who("agent-a"),
                &domain_a,
                EvidenceId::new("middle-removed").expect("id"),
                who("agent-m"),
                ExecutionRestrictions {
                    resource_domains: DomainRestriction::exact([realm("alpha")]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("removing the approval is attenuation");
        assert_eq!(
            owner
                .issue_child(
                    &meerkat_core::AdmissionGuard::none(),
                    &who("agent-m"),
                    &removed,
                    EvidenceId::new("leaf-restored").expect("id"),
                    who("agent-c"),
                    by_action(&account_a),
                )
                .err(),
            Some(GrantRefusal::Denied),
            "a grandchild cannot regain what its parent removed"
        );
        // The same account through a keeping intermediate stays delegable.
        let kept = owner
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &who("agent-a"),
                &domain_a,
                EvidenceId::new("middle-kept").expect("id"),
                who("agent-m"),
                ExecutionRestrictions::unrestricted(),
            )
            .expect("keeping the approval");
        let grandchild = owner
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &who("agent-m"),
                &kept,
                EvidenceId::new("leaf-kept").expect("id"),
                who("agent-c"),
                by_action(&account_a),
            )
            .expect("the same account through a keeping parent");
        let three = owner
            .resolve_lineage(&[domain_a.clone(), kept, grandchild], &who("agent-c"), None)
            .expect("resolve three links");

        let same = issue_child("child-action-a", &domain_a, by_action(&account_a))
            .expect("the same approval may be named again");
        let resolved = owner
            .resolve_lineage(&[domain_a, same], &who("agent-b"), None)
            .expect("resolve the child");
        let installing = |approval: &McpConnectorApproval| {
            McpInstallationRef::new(realm("alpha"), "sentinel", approval).expect("installation")
        };
        assert_eq!(
            check_resolved_installation(&resolved, &installing(&account_a), 10),
            Ok(())
        );
        assert_eq!(
            check_resolved_installation(&resolved, &installing(&account_b), 10),
            outside(RestrictionDimension::Action)
        );
        assert_eq!(
            check_resolved_installation(&three, &installing(&account_a), 10),
            Ok(())
        );

        // Resolution-time defence for chains already stored: a root without
        // an approval is missing evidence even if a later link names one, and
        // a later link naming another approval is a mismatch.
        let installation = realm("alpha");
        let connector_a = mcp_connector_action("sentinel", &account_a);
        let unrestricted = ExecutionRestrictions::unrestricted();
        assert_eq!(
            lineage_connector_bound(
                &[unrestricted, by_action(&account_a)],
                &by_action(&account_a),
                &connector_a,
                &installation
            ),
            McpConnectorBound::Unrecorded
        );
        assert_eq!(
            lineage_connector_bound(
                &[by_domain(&account_a), by_action(&account_b)],
                &by_action(&account_b),
                &mcp_connector_action("sentinel", &account_b),
                &installation
            ),
            McpConnectorBound::Other
        );
        // An intermediate link without the approval ends it, even when a
        // later link names it again.
        assert_eq!(
            lineage_connector_bound(
                &[
                    by_domain(&account_a),
                    ExecutionRestrictions {
                        resource_domains: DomainRestriction::exact([realm("alpha")]),
                        ..ExecutionRestrictions::unrestricted()
                    },
                    by_action(&account_a),
                ],
                &by_action(&account_a),
                &connector_a,
                &installation
            ),
            McpConnectorBound::Unrecorded
        );
        assert_eq!(
            lineage_connector_bound(
                &[by_domain(&account_a), by_action(&account_a)],
                &by_action(&account_a),
                &connector_a,
                &installation
            ),
            McpConnectorBound::Approved
        );
    }

    /// Equal server names under different domain authorities are distinct
    /// installations: one root may approve account aa for owner A's
    /// `sentinel` and bb for owner B's, and each pairing is checked against
    /// its own installation only.
    #[test]
    fn approvals_are_keyed_by_installation_authority_not_server_name() {
        use crate::clock::{LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError};
        use crate::grants::{LocalGrantAuthority, LocalGrantConfiguration};
        use crate::publication::LocalAuthorizationPublication;
        use meerkat_authorization_contracts::evidence::EvidenceId;

        struct At(u64);
        impl LocalAuthorizationClock for At {
            fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
                Ok(LocalAuthorizationTime {
                    unix_ms: self.0,
                    monotonic: meerkat_core::time_compat::Instant::now(),
                })
            }
        }
        let owner = LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("operator"),
                namespace: EvidenceId::new("installations").expect("id"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            std::sync::Arc::new(At(10)),
        )
        .expect("owner");
        let aa = McpConnectorApproval::Connector("sha256:aa".into());
        let bb = McpConnectorApproval::Connector("sha256:bb".into());
        let installation_of = |owner: &str| ResourceDomain {
            authority: who(owner),
            namespace: "mcp-server/sentinel".into(),
        };
        let grant = owner
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &who("operator"),
                EvidenceId::new("two-owners").expect("id"),
                who("agent"),
                None,
                ExecutionRestrictions {
                    resource_domains: DomainRestriction::exact([
                        installation_of("owner-a"),
                        mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
                        installation_of("owner-b"),
                        mcp_connector_domain(&who("owner-b"), "sentinel", &bb),
                    ]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("one root may approve each owner's installation");
        // The unqualified action form next to two owners' approvals of the
        // same server name identifies neither: refused as ambiguous.
        assert_eq!(
            owner
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &who("operator"),
                    EvidenceId::new("ambiguous-action").expect("id"),
                    who("agent"),
                    None,
                    ExecutionRestrictions {
                        actions: ExactRestriction::exact([mcp_connector_action("sentinel", &aa)]),
                        resource_domains: DomainRestriction::exact([
                            installation_of("owner-a"),
                            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
                            installation_of("owner-b"),
                            mcp_connector_domain(&who("owner-b"), "sentinel", &aa),
                        ]),
                        ..ExecutionRestrictions::unrestricted()
                    },
                )
                .err(),
            Some(crate::grants::GrantRefusal::Denied)
        );
        let resolved = owner
            .resolve_lineage(std::slice::from_ref(&grant), &who("agent"), None)
            .expect("resolve");
        let installing = |owner: &str, approval: &McpConnectorApproval| {
            McpInstallationRef::new(installation_of(owner), "sentinel", approval)
                .expect("installation")
        };
        assert_eq!(
            check_resolved_installation(&resolved, &installing("owner-a", &aa), 10),
            Ok(())
        );
        assert_eq!(
            check_resolved_installation(&resolved, &installing("owner-b", &bb), 10),
            Ok(())
        );
        for (installation, approval) in [("owner-a", &bb), ("owner-b", &aa)] {
            assert_eq!(
                check_resolved_installation(&resolved, &installing(installation, approval), 10),
                outside(RestrictionDimension::Action),
                "{installation}: the other owner's approval is not this installation's"
            );
        }

        // Narrowing the two-owner root to owner A, with A's approval named in
        // both forms, is ordinary attenuation: the child's action-form
        // approval is qualified by its own domains (owner A), not compared
        // with owner B's.
        let narrowed = owner
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &who("agent"),
                &grant,
                EvidenceId::new("narrowed-to-a").expect("id"),
                who("agent-c"),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([
                        ActionRef {
                            feature: MCP_INSTALLATION_FEATURE.to_owned(),
                            action: "sentinel".to_owned(),
                        },
                        mcp_connector_action("sentinel", &aa),
                    ]),
                    resource_domains: DomainRestriction::exact([
                        installation_of("owner-a"),
                        mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
                    ]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("narrowing to one owner's approval is attenuation");
        let resolved = owner
            .resolve_lineage(&[grant, narrowed], &who("agent-c"), None)
            .expect("resolve the narrowed child");
        assert_eq!(
            check_resolved_installation(&resolved, &installing("owner-a", &aa), 10),
            Ok(())
        );
        assert!(
            check_resolved_installation(&resolved, &installing("owner-b", &bb), 10).is_err(),
            "owner B was narrowed away"
        );
    }

    #[test]
    fn a_narrowed_child_cannot_widen_back_to_the_installation() {
        let parent = naming(&["sentinel", "other"]);
        let child = parent
            .for_child(&naming(&["other"]))
            .expect("narrowed child");
        assert_eq!(check_installation_bounds(&parent, &sentinel(), 10), Ok(()));
        assert_eq!(
            check_installation_bounds(&child, &sentinel(), 10),
            outside(RestrictionDimension::Action)
        );
    }
}
