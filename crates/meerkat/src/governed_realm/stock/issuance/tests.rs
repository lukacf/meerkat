use super::*;
use meerkat_authorization::mcp_installation::{McpInstallationRef, check_installation_bounds};
use meerkat_authorization_contracts::constraints::{
    AudienceRef, OperationRestrictionValues, RestrictionFailure,
};

fn realm() -> RealmId {
    RealmId::parse("governed").unwrap()
}

fn principals() -> StockPrincipals {
    StockPrincipals::for_user(&realm(), 4242).unwrap()
}

fn agent(name: &str) -> PrincipalRef {
    stock_agent_principal(&realm(), name).unwrap()
}

fn exact(ops: &[&str]) -> McpOperations {
    McpOperations::Exact(ops.iter().map(|op| (*op).to_string()).collect())
}

fn mcp(server: &str, operations: McpOperations) -> StockGrantTarget {
    StockGrantTarget::Mcp {
        server: server.to_string(),
        operations,
    }
}

fn restrictions(target: &StockGrantTarget) -> Result<ExecutionRestrictions, StockGovernanceError> {
    stock_grant_restrictions(
        &principals().operator,
        &agent("a"),
        target,
        Some(&meerkat_core::McpConnectorApproval::NoConnector),
        None,
        10,
    )
}

/// Whether `restrictions` cover the stock tuple of raw `op` on `server`.
fn raw_op(restrictions: &ExecutionRestrictions, server: &str, op: &str) -> bool {
    let operator = principals().operator;
    restrictions
        .check_bounds(OperationRestrictionValues {
            action: &action(MCP_FEATURE, op),
            resource_domain: &domain(&operator, format!("mcp-server/{server}")),
            processor: &ProcessorRef::Principal {
                principal: agent("a"),
            },
            audience: &AudienceRef::Principal {
                principal: operator.clone(),
            },
            now_ms: 20,
        })
        .is_ok()
}

fn stock_op(restrictions: &ExecutionRestrictions, op: &str, namespace: &str) -> bool {
    let operator = principals().operator;
    restrictions
        .check_bounds(OperationRestrictionValues {
            action: &action(STOCK_FEATURE, op),
            resource_domain: &domain(&operator, namespace.to_string()),
            processor: &ProcessorRef::Principal {
                principal: agent("a"),
            },
            audience: &AudienceRef::Principal {
                principal: operator.clone(),
            },
            now_ms: 20,
        })
        .is_ok()
}

fn entitled(restrictions: &ExecutionRestrictions, server: &str) -> bool {
    let installation = McpInstallationRef::new(
        domain(&principals().operator, format!("mcp-server/{server}")),
        server,
        &meerkat_core::McpConnectorApproval::NoConnector,
    )
    .unwrap();
    check_installation_bounds(restrictions, &installation, 20).is_ok()
}

#[test]
fn an_mcp_grant_covers_exactly_its_servers_operations_and_entitlement() {
    let granted = restrictions(&mcp("alpha", exact(&["read"]))).unwrap();
    assert!(raw_op(&granted, "alpha", "read"));
    assert!(!raw_op(&granted, "alpha", "write"), "outside the set");
    assert!(entitled(&granted, "alpha"));
    assert!(!raw_op(&granted, "beta", "read"), "another server");
    assert!(!entitled(&granted, "beta"));
    assert!(!stock_op(&granted, "catalog-read", CATALOG_NAMESPACE));
}

#[test]
fn every_operation_stays_on_its_own_server() {
    let granted = restrictions(&mcp("alpha", McpOperations::All)).unwrap();
    assert!(raw_op(&granted, "alpha", "anything"));
    assert!(entitled(&granted, "alpha"));
    assert!(!raw_op(&granted, "beta", "anything"));
    assert!(!entitled(&granted, "beta"));
    assert!(!stock_op(&granted, "catalog-read", CATALOG_NAMESPACE));
}

#[test]
fn unnamed_servers_and_empty_operations_are_refused() {
    for target in [mcp("", exact(&["read"])), mcp("alpha", exact(&[]))] {
        assert!(
            matches!(
                restrictions(&target),
                Err(StockGovernanceError::ScopeNotRepresentable(_))
            ),
            "{target:?}"
        );
    }
}

fn route(digit: char) -> ControllerRouteKey {
    ControllerRouteKey::parse(&format!(
        "{}{}",
        ControllerRouteKey::PREFIX,
        digit.to_string().repeat(64)
    ))
    .unwrap()
}

fn controller(routes: &[ControllerRouteKey]) -> StockGrantTarget {
    StockGrantTarget::Controller {
        routes: routes.iter().cloned().collect(),
    }
}

#[test]
fn controller_catalog_and_server_grants_stay_distinct() {
    let routed = route('1');
    let controller = restrictions(&controller(std::slice::from_ref(&routed))).unwrap();
    assert!(stock_op(&controller, "infer", routed.as_str()));
    assert!(!entitled(&controller, "alpha"));
    let catalog = restrictions(&StockGrantTarget::Catalog).unwrap();
    assert!(stock_op(&catalog, "catalog-read", CATALOG_NAMESPACE));
    assert!(!stock_op(&catalog, "infer", routed.as_str()));
    assert!(!entitled(&catalog, "alpha"));
    let server = restrictions(&mcp("alpha", exact(&["read"]))).unwrap();
    assert!(!stock_op(&server, "infer", routed.as_str()));
}

/// A controller grant admits exactly its named routes: another route, and
/// the bare label of a grant from before route keys, are outside it.
#[test]
fn a_controller_grant_admits_only_its_named_routes() {
    let granted = restrictions(&controller(&[route('1'), route('2')])).unwrap();
    assert!(stock_op(&granted, "infer", route('1').as_str()));
    assert!(stock_op(&granted, "infer", route('2').as_str()));
    assert!(!stock_op(&granted, "infer", route('3').as_str()));
    assert!(!stock_op(&granted, "infer", "model-route"));
    // A label-only grant admits no route.
    let operator = principals().operator;
    let label_only = ExecutionRestrictions {
        actions: ExactRestriction::exact([action(STOCK_FEATURE, "infer")]),
        resource_domains: DomainRestriction::exact([domain(&operator, "model-route".to_string())]),
        ..ExecutionRestrictions::unrestricted()
    };
    assert!(!stock_op(&label_only, "infer", route('1').as_str()));
    assert!(matches!(
        restrictions(&controller(&[])),
        Err(StockGovernanceError::ScopeNotRepresentable(_))
    ));
}

#[test]
fn another_executor_and_an_expired_window_are_outside() {
    let granted = stock_grant_restrictions(
        &principals().operator,
        &agent("a"),
        &mcp("alpha", exact(&["read"])),
        Some(&meerkat_core::McpConnectorApproval::NoConnector),
        Some(5),
        10,
    )
    .unwrap();
    let operator = principals().operator;
    let check = |processor: PrincipalRef, now_ms| {
        granted.check_bounds(OperationRestrictionValues {
            action: &action(MCP_FEATURE, "read"),
            resource_domain: &domain(&operator, "mcp-server/alpha".to_string()),
            processor: &ProcessorRef::Principal {
                principal: processor,
            },
            audience: &AudienceRef::Principal {
                principal: operator.clone(),
            },
            now_ms,
        })
    };
    assert_eq!(check(agent("a"), 12), Ok(()));
    assert!(matches!(
        check(agent("b"), 12),
        Err(RestrictionFailure::OutsideBounds { .. })
    ));
    assert!(matches!(
        check(agent("a"), 15),
        Err(RestrictionFailure::OutsideBounds { .. })
    ));
}

fn bundle_operation(bundle: &str, operation: &str) -> StockGrantTarget {
    StockGrantTarget::ToolBundleOperation {
        bundle: bundle.to_string(),
        operation: operation.to_string(),
    }
}

fn bundle_slot(bundle: &str, operation: &str) -> StockGrantSlot {
    StockGrantSlot::ToolBundleOperation {
        bundle: bundle.to_string(),
        operation: operation.to_string(),
    }
}

/// Whether `restrictions` cover operation `op` of the registered bundle
/// `bundle` at `now_ms`.
fn bundle_op(restrictions: &ExecutionRestrictions, bundle: &str, op: &str, now_ms: u64) -> bool {
    let operator = principals().operator;
    restrictions
        .check_bounds(OperationRestrictionValues {
            action: &action(TOOL_BUNDLE_FEATURE, op),
            resource_domain: &domain(&operator, tool_bundle_namespace(bundle)),
            processor: &ProcessorRef::Principal {
                principal: agent("a"),
            },
            audience: &AudienceRef::Principal {
                principal: operator.clone(),
            },
            now_ms,
        })
        .is_ok()
}

/// A tool bundle operation grant covers exactly that operation of that
/// bundle, within its own window, and nothing of the catalog, an MCP server
/// or the controller; unnamed bundles and operations are refused.
#[test]
fn a_tool_bundle_grant_covers_exactly_its_operation() {
    let granted = stock_grant_restrictions(
        &principals().operator,
        &agent("a"),
        &bundle_operation("mail", "read_mail"),
        None,
        Some(5),
        10,
    )
    .unwrap();
    assert!(bundle_op(&granted, "mail", "read_mail", 12));
    assert!(!bundle_op(&granted, "mail", "read_mail", 15), "expired");
    assert!(!bundle_op(&granted, "mail", "send_draft", 12), "sibling");
    assert!(
        !bundle_op(&granted, "other", "read_mail", 12),
        "other bundle"
    );
    assert!(!stock_op(&granted, "catalog-read", CATALOG_NAMESPACE));
    assert!(!raw_op(&granted, "mail", "read_mail"));
    assert!(!entitled(&granted, "mail"));
    let catalog = restrictions(&StockGrantTarget::Catalog).unwrap();
    assert!(
        !bundle_op(&catalog, "mail", "read_mail", 12),
        "a catalog grant never covers a bundle operation"
    );
    for target in [
        bundle_operation("", "read_mail"),
        bundle_operation("mail", ""),
    ] {
        assert!(
            matches!(
                restrictions(&target),
                Err(StockGovernanceError::ScopeNotRepresentable(_))
            ),
            "{target:?}"
        );
    }
}

#[test]
fn a_controller_grant_with_an_expiry_is_refused() {
    assert!(matches!(
        stock_grant_restrictions(
            &principals().operator,
            &agent("a"),
            &controller(&[route('1')]),
            None,
            Some(5),
            10,
        ),
        Err(StockGovernanceError::ScopeNotRepresentable(_))
    ));
}

/// Two issued references from one in-memory owner.
fn refs(ids: &[&str]) -> Vec<GrantLineageRef> {
    let owner = LocalGrantAuthority::new(
        meerkat_authorization::grants::LocalGrantConfiguration {
            root: principals().operator,
            namespace: EvidenceId::new("refs").unwrap(),
            generation: 1,
        },
        LocalAuthorizationPublication::new(),
        Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
    )
    .unwrap();
    ids.iter()
        .map(|id| {
            owner
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &principals().operator,
                    EvidenceId::new(*id).unwrap(),
                    agent("a"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap()
        })
        .collect()
}

#[test]
fn the_selection_file_reads_the_legacy_forms_and_selects_per_server() {
    let [operation, controller, alpha, catalog]: [GrantLineageRef; 4] =
        refs(&["worker", "controller", "alpha", "catalog"])
            .try_into()
            .unwrap();
    let legacy = serde_json::json!({ "operation": operation, "controller": controller });
    let file: StockGrantFile = serde_json::from_value(legacy).unwrap();
    let older = &file.agents[DEFAULT_STOCK_AGENT];
    assert_eq!(older.operation.as_ref(), Some(&operation));
    assert_eq!(older.controller.as_ref(), Some(&controller));
    // An older selection's single worker grant stands in for every target,
    // and is the whole work basis, the catalog's base work included.
    assert_eq!(older.server_lineage("alpha"), Some(&operation));
    assert_eq!(older.server_lineage("beta"), Some(&operation));
    assert_eq!(
        older.prompt_selection().map(|(basis, _)| basis),
        Some(WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation]
        })
    );

    // Once a per-target grant exists, each target has only its own.
    let mut current = older.clone();
    current.mcp.insert("alpha".to_string(), alpha.clone());
    assert_eq!(current.server_lineage("alpha"), Some(&alpha));
    assert_eq!(current.server_lineage("beta"), None);
    assert!(matches!(
        current.prompt_selection(),
        Some((
            WorkAuthorityBasis::TargetedGrantLineages { base: None, .. },
            _
        ))
    ));
    current.catalog = Some(catalog.clone());
    assert!(matches!(
        current.prompt_selection(),
        Some((WorkAuthorityBasis::TargetedGrantLineages { base: Some(base), .. }, _))
            if base == std::slice::from_ref(&catalog)
    ));
    current.replace(&bundle_slot("mail", "read_mail"), alpha.clone());
    assert_eq!(current.get(&bundle_slot("mail", "read_mail")), Some(&alpha));

    let file = StockGrantFile {
        agents: BTreeMap::from([("a".to_string(), current)]),
    };
    let round_trip: StockGrantFile =
        serde_json::from_slice(&serde_json::to_vec(&file).unwrap()).unwrap();
    assert_eq!(round_trip, file);
}

/// A disk realm the local operator governed; nothing is issued.
async fn governed_realm() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    drop(
        super::super::super::open_governed_realm_persistence_with_provider(
            &crate::DiskStorageProvider,
            tmp.path(),
            "governed",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            super::super::super::RealmGovernance::Ungoverned,
        )
        .await
        .expect("an ordinary realm"),
    );
    // The realm's own installations every disk fixture may grant.
    let paths = meerkat_store::realm_paths_in(tmp.path(), "governed");
    let runtime = meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    );
    for server in ["alpha", "beta"] {
        meerkat_core::McpRealmPersistTarget::new(&runtime)
            .add(
                meerkat_core::McpServerConfig::stdio(
                    server,
                    "true",
                    Vec::new(),
                    std::collections::HashMap::new(),
                ),
                None,
            )
            .await
            .expect("installed");
    }
    super::super::govern_stock_realm(tmp.path(), "governed", &principals())
        .await
        .expect("governed");
    tmp
}

/// The bytes of the realm's grant owner and grant selection.
fn realm_state(root: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    let paths = meerkat_store::realm_paths_in(root, "governed");
    (
        std::fs::read(meerkat_store::grant_owner_store_path(&paths.root)).unwrap(),
        std::fs::read(super::super::stock_grants_path(&paths)).ok(),
    )
}

async fn selection(root: &Path) -> StockGrantFile {
    let paths = meerkat_store::realm_paths_in(root, "governed");
    super::super::read_stock_grant_file(&paths)
        .await
        .unwrap()
        .unwrap_or_default()
}

fn request(agent: &str, target: StockGrantTarget) -> StockGrantRequest {
    StockGrantRequest {
        agent: agent.to_string(),
        target,
        expires_in_ms: None,
    }
}

fn resolves(owner: &LocalGrantAuthority, grant: &GrantLineageRef, as_agent: &str) -> bool {
    owner
        .resolve_lineage(
            std::slice::from_ref(grant),
            &stock_agent_principal(&realm(), as_agent).unwrap(),
            None,
        )
        .is_ok()
}

/// Two servers with disjoint operations get two exact grants; replacing one
/// supersedes only it (recorded for cleanup, still live while its cleanup is
/// deferred) and leaves the other selected; an explicit revoke of one is
/// immediate and unselects only it.
#[tokio::test]
async fn per_server_grants_are_issued_replaced_and_revoked_independently() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    assert!(
        selection(root).await.agents.is_empty(),
        "governing issues nothing"
    );

    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("alpha");
    let beta = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", mcp("beta", exact(&["write"]))),
    )
    .await
    .expect("beta");
    assert!(alpha.superseded.is_none() && beta.superseded.is_none());
    let file = selection(root).await;
    let refs = &file.agents["agent-a"];
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha.issued));
    assert_eq!(refs.server_lineage("beta"), Some(&beta.issued));
    let owner = restore_owner(root, "governed").unwrap();
    let alpha_grant = owner
        .resolve_lineage(
            std::slice::from_ref(&alpha.issued),
            &agent_principal("agent-a"),
            None,
        )
        .unwrap();
    assert!(raw_op_for(alpha_grant.restrictions(), "alpha", "read"));
    assert!(!raw_op_for(alpha_grant.restrictions(), "beta", "write"));
    assert!(
        !resolves(&owner, &alpha.issued, "agent-b"),
        "B cannot use A's grant"
    );
    drop(owner);

    // Replacing alpha supersedes only alpha's grant; with no unfinished work
    // referencing it, its cleanup revokes it right away.
    let replaced = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", mcp("alpha", McpOperations::All)),
    )
    .await
    .expect("replacement");
    assert_eq!(replaced.superseded.as_ref(), Some(&alpha.issued));
    assert!(matches!(
        replaced.cleanup.as_slice(),
        [CleanupAttempt { outcome: RevocationOutcome::Revoked, grant, .. }]
            if grant == &alpha.issued
    ));
    let file = selection(root).await;
    let refs = &file.agents["agent-a"];
    assert_eq!(refs.server_lineage("alpha"), Some(&replaced.issued));
    assert_eq!(refs.server_lineage("beta"), Some(&beta.issued));
    assert!(refs.pending_cleanup.is_empty(), "retired");
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &alpha.issued, "agent-a"), "retired");
    assert!(resolves(&owner, &replaced.issued, "agent-a"));
    assert!(resolves(&owner, &beta.issued, "agent-a"), "beta untouched");
    drop(owner);

    // An explicit revoke of beta is immediate and unselects only beta.
    let revoked = revoke_stock_agent(
        root,
        "governed",
        &principals,
        "agent-a",
        &StockGrantSlot::Mcp("beta".to_string()),
    )
    .await
    .expect("revoke");
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
    assert_eq!(revoked.revoked, beta.issued);
    assert!(revoked.unselected.is_ok());
    let file = selection(root).await;
    let refs = &file.agents["agent-a"];
    assert_eq!(refs.server_lineage("beta"), None);
    assert_eq!(refs.server_lineage("alpha"), Some(&replaced.issued));
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &beta.issued, "agent-a"));
    assert!(resolves(&owner, &replaced.issued, "agent-a"));
    drop(owner);

    assert!(matches!(
        revoke_stock_agent(
            root,
            "governed",
            &principals,
            "agent-a",
            &StockGrantSlot::Mcp("beta".to_string()),
        )
        .await,
        Err(StockGovernanceError::GrantsUnavailable)
    ));
}

fn bundle_target(bundle: &str, operation: &str) -> GrantTarget {
    GrantTarget::RegisteredToolBundleOperation {
        bundle: bundle.to_string(),
        operation: operation.to_string(),
    }
}

/// Each tool bundle operation grant is issued and selected on its own,
/// beside the catalog, an MCP server and the controller: the prompt basis
/// gives it its own exact target lineage (in one canonical order with the
/// server's), never the catalog base. The selection a later process reads
/// and the restored grant owner keep it; revoking one operation unselects
/// only it.
#[tokio::test]
async fn bundle_operation_grants_are_selected_independently_and_retained_across_reopen() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let grant = |target: StockGrantTarget| {
        let principals = principals.clone();
        async move {
            grant_stock_agent(root, "governed", &principals, &request("agent-a", target))
                .await
                .expect("issued and selected")
                .issued
        }
    };
    let catalog = grant(StockGrantTarget::Catalog).await;
    let alpha = grant(mcp("alpha", exact(&["read"]))).await;
    let send = grant(bundle_operation("mail", "send_draft")).await;
    let read = grant(bundle_operation("mail", "read_mail")).await;
    let controller_grant = grant(controller(&[route('1')])).await;

    // What a reopened realm reads, and what its restored owner resolves.
    let reopened = selection(root).await;
    let refs = &reopened.agents["agent-a"];
    assert_eq!(refs.get(&bundle_slot("mail", "read_mail")), Some(&read));
    assert_eq!(refs.get(&bundle_slot("mail", "send_draft")), Some(&send));
    let (basis, selected_controller) = refs.prompt_selection().expect("selected");
    assert_eq!(selected_controller, controller_grant);
    assert_eq!(
        basis,
        WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(vec![catalog.clone()]),
            targets: vec![
                TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "alpha".to_string(),
                    },
                    lineage: vec![alpha.clone()],
                },
                TargetGrantLineage {
                    target: bundle_target("mail", "read_mail"),
                    lineage: vec![read.clone()],
                },
                TargetGrantLineage {
                    target: bundle_target("mail", "send_draft"),
                    lineage: vec![send.clone()],
                },
            ],
        }
    );
    assert_eq!(
        basis.lineage_for(Some(&bundle_target("mail", "list_mail"))),
        None,
        "an operation without its own grant has no lineage"
    );
    let owner = restore_owner(root, "governed").unwrap();
    let resolved = owner
        .resolve_lineage(
            std::slice::from_ref(&read),
            &agent_principal("agent-a"),
            None,
        )
        .unwrap();
    let operator = principals.operator.clone();
    let covers = |op: &str| {
        resolved
            .restrictions()
            .check_bounds(OperationRestrictionValues {
                action: &action(TOOL_BUNDLE_FEATURE, op),
                resource_domain: &domain(&operator, tool_bundle_namespace("mail")),
                processor: &ProcessorRef::Principal {
                    principal: agent_principal("agent-a"),
                },
                audience: &AudienceRef::Principal {
                    principal: operator.clone(),
                },
                now_ms: 20,
            })
            .is_ok()
    };
    assert!(covers("read_mail"));
    assert!(!covers("send_draft"));
    assert!(
        !resolves(&owner, &read, "agent-b"),
        "B cannot use A's grant"
    );
    drop(owner);

    // Revoking one operation's grant unselects only it.
    let revoked = revoke_stock_agent(
        root,
        "governed",
        &principals,
        "agent-a",
        &bundle_slot("mail", "send_draft"),
    )
    .await
    .expect("revoke");
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
    assert_eq!(revoked.revoked, send);
    let refs = &selection(root).await.agents["agent-a"];
    assert_eq!(refs.get(&bundle_slot("mail", "send_draft")), None);
    assert_eq!(refs.get(&bundle_slot("mail", "read_mail")), Some(&read));
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    assert_eq!(refs.get(&StockGrantSlot::Catalog), Some(&catalog));
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &send, "agent-a"));
    assert!(resolves(&owner, &read, "agent-a"));
}

fn checked_route_target(route: &str, server: &str, operations: &[&str]) -> StockGrantTarget {
    StockGrantTarget::CheckedMcpRoute {
        route: route.to_string(),
        server: server.to_string(),
        operations: operations.iter().map(|op| (*op).to_string()).collect(),
    }
}

/// A checked route grant has the exact shape of an MCP server grant of its
/// operations and is refused without a route, a server or exact
/// operations.
#[test]
fn a_checked_route_grant_covers_exactly_its_operations_on_its_server() {
    let granted = restrictions(&checked_route_target("checked", "alpha", &["install"])).unwrap();
    assert!(raw_op(&granted, "alpha", "install"));
    assert!(!raw_op(&granted, "alpha", "read"), "outside the route");
    assert!(entitled(&granted, "alpha"));
    assert!(!raw_op(&granted, "beta", "install"), "another server");
    for target in [
        checked_route_target("", "alpha", &["install"]),
        checked_route_target("checked", "", &["install"]),
        checked_route_target("checked", "alpha", &[]),
        checked_route_target("checked", "alpha", &[""]),
    ] {
        assert!(
            matches!(
                restrictions(&target),
                Err(StockGovernanceError::ScopeNotRepresentable(_))
            ),
            "{target:?}"
        );
    }
}

/// A checked route's grant is issued from the realm's own installation and
/// selected for exactly that route to that server, beside the server's own
/// grant: the prompt basis gives each its own target lineage, and revoking
/// the route's grant unselects only it.
#[tokio::test]
async fn checked_route_grants_are_selected_beside_the_servers_own_grant() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let grant = |target: StockGrantTarget| {
        let principals = principals.clone();
        async move {
            grant_stock_agent(root, "governed", &principals, &request("agent-a", target))
                .await
                .expect("issued and selected")
                .issued
        }
    };
    let alpha = grant(mcp("alpha", exact(&["read"]))).await;
    let checked = grant(checked_route_target("checked", "alpha", &["install"])).await;
    let controller_grant = grant(controller(&[route('1')])).await;
    let slot = StockGrantSlot::CheckedMcpRoute {
        route: "checked".to_string(),
        server: "alpha".to_string(),
    };

    let refs = &selection(root).await.agents["agent-a"];
    assert_eq!(refs.get(&slot), Some(&checked));
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    let (basis, selected_controller) = refs.prompt_selection().expect("selected");
    assert_eq!(selected_controller, controller_grant);
    assert_eq!(
        basis,
        WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![
                TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "alpha".to_string(),
                    },
                    lineage: vec![alpha.clone()],
                },
                TargetGrantLineage {
                    target: GrantTarget::CheckedMcpRoute {
                        route: "checked".to_string(),
                        server: "alpha".to_string(),
                    },
                    lineage: vec![checked.clone()],
                },
            ],
        }
    );
    // A checked route to an uninstalled server is refused before any effect.
    assert!(
        grant_stock_agent(
            root,
            "governed",
            &principals,
            &request(
                "agent-a",
                checked_route_target("checked", "gamma", &["install"])
            ),
        )
        .await
        .is_err()
    );

    let revoked = revoke_stock_agent(root, "governed", &principals, "agent-a", &slot)
        .await
        .expect("revoke");
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
    assert_eq!(revoked.revoked, checked);
    let refs = &selection(root).await.agents["agent-a"];
    assert_eq!(refs.get(&slot), None);
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &checked, "agent-a"));
    assert!(resolves(&owner, &alpha, "agent-a"));
}

/// Removing the last tool bundle operation grant of an agent whose older
/// worker grant it superseded revives neither that grant nor the catalog
/// base for the operation: the prompt basis stays targeted and the
/// operation has no lineage, so it stays denied.
#[tokio::test]
async fn removing_the_last_bundle_operation_grant_revives_no_other_grant() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let owner = restore_owner(root, "governed").unwrap();
    let issue = |id: &str| {
        owner
            .issue_root(
                &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                    .expect("the grant log's fence"),
                &principals.operator,
                EvidenceId::new(id).unwrap(),
                agent_principal(DEFAULT_STOCK_AGENT),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap()
    };
    let legacy = super::super::StockGrantSelection {
        operation: issue("legacy-worker"),
        controller: issue("legacy-controller"),
    };
    drop(owner);
    super::super::record_stock_grant_selection(root, "governed", &legacy)
        .await
        .expect("older selection");
    let read = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, bundle_operation("mail", "read_mail")),
    )
    .await
    .expect("the first per-target grant");
    assert!(
        read.cleanup
            .iter()
            .any(|attempt| attempt.grant == legacy.operation),
        "the older worker grant is recorded for cleanup"
    );
    let catalog = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, StockGrantTarget::Catalog),
    )
    .await
    .expect("catalog");
    revoke_stock_agent(
        root,
        "governed",
        &principals,
        DEFAULT_STOCK_AGENT,
        &bundle_slot("mail", "read_mail"),
    )
    .await
    .expect("revoke the last bundle operation grant");

    let reopened = selection(root).await;
    let refs = &reopened.agents[DEFAULT_STOCK_AGENT];
    assert!(refs.tool_bundles.is_empty());
    assert_eq!(refs.operation, None);
    let (basis, _) = refs.prompt_selection().expect("selected");
    assert_eq!(
        basis,
        WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(vec![catalog.issued]),
            targets: Vec::new(),
        },
        "targeted, never the older grant"
    );
    assert_eq!(
        basis.lineage_for(Some(&bundle_target("mail", "read_mail"))),
        None,
        "the operation has no lineage: neither the base nor the older grant"
    );
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &read.issued, DEFAULT_STOCK_AGENT));
    assert!(!resolves(&owner, &legacy.operation, DEFAULT_STOCK_AGENT));
}

fn agent_principal(name: &str) -> PrincipalRef {
    stock_agent_principal(&realm(), name).unwrap()
}

fn raw_op_for(restrictions: &ExecutionRestrictions, server: &str, op: &str) -> bool {
    let operator = principals().operator;
    restrictions
        .check_bounds(OperationRestrictionValues {
            action: &action(MCP_FEATURE, op),
            resource_domain: &domain(&operator, format!("mcp-server/{server}")),
            processor: &ProcessorRef::Principal {
                principal: agent_principal("agent-a"),
            },
            audience: &AudienceRef::Principal {
                principal: operator.clone(),
            },
            now_ms: 20,
        })
        .is_ok()
}

/// The realm's exclusive custody serializes selection updates: of two
/// concurrent grants, each either completes or is refused before any effect,
/// and no completed one is lost.
#[tokio::test]
async fn concurrent_selection_updates_never_lose_one() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = request("agent-a", mcp("alpha", exact(&["read"])));
    let beta = request("agent-a", mcp("beta", exact(&["read"])));
    let (first, second) = tokio::join!(
        grant_stock_agent(root, "governed", &principals, &alpha),
        grant_stock_agent(root, "governed", &principals, &beta),
    );
    for outcome in [&first, &second] {
        assert!(
            matches!(outcome, Ok(_) | Err(StockGrantError::Refused(_))),
            "{outcome:?}"
        );
    }
    let file = selection(root).await;
    let refs = file.agents.get("agent-a").cloned().unwrap_or_default();
    for (server, outcome) in [("alpha", &first), ("beta", &second)] {
        match outcome {
            Ok(outcome) => assert_eq!(refs.server_lineage(server), Some(&outcome.issued)),
            Err(_) => assert_eq!(refs.server_lineage(server), None),
        }
    }
    // A refused one succeeds once the other is done.
    for (server, request, outcome) in [("alpha", &alpha, &first), ("beta", &beta, &second)] {
        if outcome.is_err() {
            grant_stock_agent(root, "governed", &principals, request)
                .await
                .expect("retried");
            assert!(
                selection(root).await.agents["agent-a"]
                    .server_lineage(server)
                    .is_some()
            );
        }
    }
}

#[tokio::test]
async fn a_controller_expiry_is_refused_before_any_effect() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let controller = |expires_in_ms| StockGrantRequest {
        agent: "agent-a".to_string(),
        target: controller(&[route('1')]),
        expires_in_ms,
    };
    let usable = grant_stock_agent(root, "governed", &principals, &controller(None))
        .await
        .expect("an unrestricted controller grant")
        .issued;
    let before = realm_state(root);

    assert!(matches!(
        grant_stock_agent(root, "governed", &principals, &controller(Some(5_000))).await,
        Err(StockGrantError::Refused(
            StockGovernanceError::ScopeNotRepresentable(_)
        ))
    ));
    assert_eq!(
        realm_state(root),
        before,
        "nothing issued, selected or revoked"
    );
    restore_owner(root, "governed")
        .unwrap()
        .resolve_controller_lineage(
            std::slice::from_ref(&usable),
            &stock_agent_principal(&realm(), "agent-a").unwrap(),
            None,
        )
        .expect("the usable controller grant is still selected and live");
}

#[tokio::test]
async fn an_unreadable_selection_is_refused_before_issuing() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let paths = meerkat_store::realm_paths_in(root, "governed");
    std::fs::write(super::super::stock_grants_path(&paths), b"not a selection").unwrap();
    let before = realm_state(root);
    let outcome = grant_stock_agent(
        root,
        "governed",
        &principals(),
        &request("agent-a", StockGrantTarget::Catalog),
    )
    .await;
    assert!(
        matches!(
            outcome,
            Err(StockGrantError::Refused(
                StockGovernanceError::GrantsUnavailable
            ))
        ),
        "{outcome:?}"
    );
    assert_eq!(realm_state(root), before, "no grant was issued");
}

/// Injected effects, recording which ran and every written selection.
struct Effects {
    issue: Result<GrantLineageRef, GrantRefusal>,
    /// `Some(true)`: the first write stops after the rename; `Some(false)`:
    /// before.
    select_failure: Option<bool>,
    retire: RevocationOutcome,
    revoke: RevocationOutcome,
    ran: std::sync::Mutex<Vec<&'static str>>,
    written: std::sync::Mutex<Vec<StockGrantFile>>,
}

impl Effects {
    fn new(issue: Result<GrantLineageRef, GrantRefusal>) -> Self {
        Self {
            issue,
            select_failure: None,
            retire: RevocationOutcome::Revoked,
            revoke: RevocationOutcome::Revoked,
            ran: std::sync::Mutex::new(Vec::new()),
            written: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn ran(&self) -> Vec<&'static str> {
        self.ran.lock().unwrap().clone()
    }

    fn written(&self) -> Vec<StockGrantFile> {
        self.written.lock().unwrap().clone()
    }
}

impl IssuanceEffects for Effects {
    fn issue(
        &self,
        _id: EvidenceId,
        _executor: PrincipalRef,
        _restrictions: ExecutionRestrictions,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        self.ran.lock().unwrap().push("issue");
        self.issue.clone()
    }

    async fn select(
        &self,
        file: &StockGrantFile,
    ) -> Result<(), super::super::SelectionWriteFailure> {
        self.ran.lock().unwrap().push("select");
        let error = || std::io::Error::other("injected");
        let first = self.written.lock().unwrap().is_empty();
        match self.select_failure.filter(|_| first) {
            None => {
                self.written.lock().unwrap().push(file.clone());
                Ok(())
            }
            Some(false) => Err(super::super::SelectionWriteFailure::NotWritten(error())),
            Some(true) => {
                self.written.lock().unwrap().push(file.clone());
                Err(super::super::SelectionWriteFailure::NotDurable(error()))
            }
        }
    }

    fn retire(&self, _grant: &GrantLineageRef) -> RevocationOutcome {
        self.ran.lock().unwrap().push("retire");
        self.retire
    }

    fn revoke(&self, _grant: &GrantLineageRef) -> RevocationOutcome {
        self.ran.lock().unwrap().push("revoke");
        self.revoke
    }
}

fn selecting(server: &str, grant: &GrantLineageRef) -> StockGrantFile {
    StockGrantFile {
        agents: BTreeMap::from([(
            "a".to_string(),
            AgentGrantRefs {
                mcp: BTreeMap::from([(server.to_string(), grant.clone())]),
                ..AgentGrantRefs::default()
            },
        )]),
    }
}

async fn run(
    effects: &Effects,
    file: StockGrantFile,
    server: &str,
) -> Result<StockGrantOutcome, StockGrantError> {
    issue_and_select(
        effects,
        file,
        "a",
        &StockGrantSlot::Mcp(server.to_string()),
        EvidenceId::new("attempt").unwrap(),
        agent("a"),
        ExecutionRestrictions::unrestricted(),
    )
    .await
}

#[tokio::test]
async fn a_failed_issuance_is_uncertain_or_refused_and_selects_nothing() {
    let [old]: [GrantLineageRef; 1] = refs(&["old"]).try_into().unwrap();
    let effects = Effects::new(Err(GrantRefusal::Unavailable));
    assert!(matches!(
        run(&effects, selecting("alpha", &old), "alpha").await,
        Err(StockGrantError::IssuanceUncertain { id }) if id.as_str() == "attempt"
    ));
    assert_eq!(effects.ran(), ["issue"]);

    let effects = Effects::new(Err(GrantRefusal::Denied));
    assert!(matches!(
        run(&effects, selecting("alpha", &old), "alpha").await,
        Err(StockGrantError::Refused(StockGovernanceError::GrantOwner(
            _
        )))
    ));
    assert_eq!(effects.ran(), ["issue"]);
}

#[tokio::test]
async fn an_unwritten_selection_keeps_the_issued_reference_and_retires_nothing() {
    let [old, new]: [GrantLineageRef; 2] = refs(&["old", "new"]).try_into().unwrap();
    let mut effects = Effects::new(Ok(new.clone()));
    effects.select_failure = Some(false);
    assert!(matches!(
        run(&effects, selecting("alpha", &old), "alpha").await,
        Err(StockGrantError::NotSelected { issued, .. }) if issued == new
    ));
    assert_eq!(effects.ran(), ["issue", "select"]);
}

#[tokio::test]
async fn a_selection_that_may_not_be_durable_retires_nothing() {
    let [old, new]: [GrantLineageRef; 2] = refs(&["old", "new"]).try_into().unwrap();
    let mut effects = Effects::new(Ok(new.clone()));
    effects.select_failure = Some(true);
    assert!(matches!(
        run(&effects, selecting("alpha", &old), "alpha").await,
        Err(StockGrantError::SelectionUncertain { issued, previous: Some(previous), .. })
            if issued == new && previous == old
    ));
    assert_eq!(
        effects.ran(),
        ["issue", "select"],
        "the old grant stays live"
    );
}

/// The superseded reference is recorded for cleanup in the same write that
/// selects the new grant, before any retirement is attempted.
#[tokio::test]
async fn the_superseded_grant_is_recorded_with_the_new_selection_before_retirement() {
    let [old, new, other]: [GrantLineageRef; 3] =
        refs(&["old", "new", "other"]).try_into().unwrap();
    let mut file = selecting("alpha", &old);
    file.agents
        .get_mut("a")
        .unwrap()
        .mcp
        .insert("beta".to_string(), other.clone());
    let mut effects = Effects::new(Ok(new.clone()));
    effects.retire = RevocationOutcome::Deferred;
    let outcome = run(&effects, file, "alpha")
        .await
        .expect("issued and selected");
    assert_eq!(outcome.superseded.as_ref(), Some(&old));
    assert!(matches!(
        outcome.cleanup.as_slice(),
        [CleanupAttempt { outcome: RevocationOutcome::Deferred, grant, .. }] if grant == &old
    ));
    assert_eq!(effects.ran(), ["issue", "select", "retire"]);
    let written = effects.written();
    let [selected] = written.as_slice() else {
        panic!("one write: {written:?}");
    };
    let refs = &selected.agents["a"];
    assert_eq!(refs.mcp["alpha"], new);
    assert_eq!(refs.mcp["beta"], other, "the other server is untouched");
    assert_eq!(refs.pending_cleanup, vec![old]);
}

#[tokio::test]
async fn a_retired_grant_leaves_the_cleanup_record_in_a_second_write() {
    let [old, new]: [GrantLineageRef; 2] = refs(&["old", "new"]).try_into().unwrap();
    let effects = Effects::new(Ok(new.clone()));
    let outcome = run(&effects, selecting("alpha", &old), "alpha")
        .await
        .expect("issued and selected");
    assert!(matches!(
        outcome.cleanup.as_slice(),
        [CleanupAttempt {
            outcome: RevocationOutcome::Revoked,
            ..
        }]
    ));
    assert_eq!(effects.ran(), ["issue", "select", "retire", "select"]);
    let written = effects.written();
    assert_eq!(written[0].agents["a"].pending_cleanup, vec![old]);
    assert!(written[1].agents["a"].pending_cleanup.is_empty());
    assert_eq!(written[1].agents["a"].mcp["alpha"], new);
}

#[tokio::test]
async fn an_explicit_revoke_unselects_only_a_revoked_grant() {
    let [granted]: [GrantLineageRef; 1] = refs(&["granted"]).try_into().unwrap();
    let mut effects = Effects::new(Err(GrantRefusal::Denied));
    effects.revoke = RevocationOutcome::Uncertain;
    let outcome = revoke_and_unselect(
        &effects,
        selecting("alpha", &granted),
        "a",
        &StockGrantSlot::Mcp("alpha".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(outcome.outcome, RevocationOutcome::Uncertain);
    assert_eq!(effects.ran(), ["revoke"], "the selection is unchanged");

    let effects = Effects::new(Err(GrantRefusal::Denied));
    let outcome = revoke_and_unselect(
        &effects,
        selecting("alpha", &granted),
        "a",
        &StockGrantSlot::Mcp("alpha".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(outcome.outcome, RevocationOutcome::Revoked);
    assert!(outcome.unselected.is_ok());
    assert_eq!(effects.ran(), ["revoke", "select"]);
    assert!(effects.written()[0].agents["a"].mcp.is_empty());
}

#[test]
fn revocation_outcomes_are_typed_by_what_is_known() {
    assert_eq!(revocation_outcome(Ok(())), RevocationOutcome::Revoked);
    assert_eq!(
        revocation_outcome(Err(GrantRefusal::ControllerInUse)),
        RevocationOutcome::ControllerInUse
    );
    assert_eq!(
        revocation_outcome(Err(GrantRefusal::Denied)),
        RevocationOutcome::Refused
    );
    assert_eq!(
        revocation_outcome(Err(GrantRefusal::Unavailable)),
        RevocationOutcome::Uncertain
    );
    assert_eq!(
        custody_refusal_outcome(ControllerCustodyRefusal::ControllerInUse),
        RevocationOutcome::ControllerInUse
    );
    assert_eq!(
        custody_refusal_outcome(ControllerCustodyRefusal::Unavailable),
        RevocationOutcome::CustodyUnavailable
    );
}

/// Once any per-server grant is selected, an older selection's single worker
/// grant no longer fills gaps: a server without its own grant is refused.
#[test]
fn a_per_server_selection_never_falls_back_to_the_older_worker_grant() {
    let [operation, controller, alpha]: [GrantLineageRef; 3] =
        refs(&["worker", "controller", "alpha"]).try_into().unwrap();
    let mut refs = AgentGrantRefs {
        operation: Some(operation.clone()),
        controller: Some(controller.clone()),
        ..AgentGrantRefs::default()
    };
    assert_eq!(refs.server_lineage("beta"), Some(&operation), "pure legacy");
    refs.mcp.insert("alpha".to_string(), alpha.clone());
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    assert_eq!(refs.server_lineage("beta"), None);
    let (basis, selected_controller) = refs.prompt_selection().expect("selected");
    assert_eq!(selected_controller, controller);
    assert_eq!(
        basis,
        WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: "alpha".to_string(),
                },
                lineage: vec![alpha],
            }],
        }
    );
    assert_eq!(
        basis.lineage_for(Some(&GrantTarget::McpServer {
            server: "beta".to_string()
        })),
        None,
        "no lineage for an unselected server"
    );
}

#[test]
fn an_older_selection_keeps_its_single_lineage_basis() {
    let [operation, controller]: [GrantLineageRef; 2] =
        refs(&["worker", "controller"]).try_into().unwrap();
    let refs = AgentGrantRefs {
        operation: Some(operation.clone()),
        controller: Some(controller),
        ..AgentGrantRefs::default()
    };
    assert_eq!(
        refs.prompt_selection().map(|(basis, _)| basis),
        Some(WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation]
        })
    );
    assert!(
        AgentGrantRefs {
            controller: None,
            ..refs
        }
        .prompt_selection()
        .is_none(),
        "no controller, no prompts"
    );
}

/// The first per-target grant records the older single worker grant for
/// cleanup in the same write that selects the new grant: it is never
/// selected again.
#[tokio::test]
async fn adopting_a_per_target_grant_retires_the_older_worker_grant_in_the_same_write() {
    let [legacy, new]: [GrantLineageRef; 2] = refs(&["legacy", "new"]).try_into().unwrap();
    let file = StockGrantFile {
        agents: BTreeMap::from([(
            "a".to_string(),
            AgentGrantRefs {
                operation: Some(legacy.clone()),
                ..AgentGrantRefs::default()
            },
        )]),
    };
    let mut effects = Effects::new(Ok(new.clone()));
    effects.retire = RevocationOutcome::Deferred;
    let outcome = run(&effects, file, "alpha")
        .await
        .expect("issued and selected");
    assert!(matches!(
        outcome.cleanup.as_slice(),
        [CleanupAttempt { outcome: RevocationOutcome::Deferred, grant, .. }] if grant == &legacy
    ));
    let written = effects.written();
    let refs = &written[0].agents["a"];
    assert_eq!(refs.operation, None);
    assert_eq!(refs.pending_cleanup, vec![legacy]);
    assert_eq!(refs.server_lineage("alpha"), Some(&new));
    assert_eq!(refs.server_lineage("beta"), None);
}

/// Live older selection, then one per-target grant, then its revocation:
/// a freshly read selection never falls back to the older worker grant, and
/// the prompt basis is the empty targeted one.
#[tokio::test]
async fn removing_the_last_target_never_resurrects_the_older_worker_grant() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let owner = restore_owner(root, "governed").unwrap();
    let issue = |id: &str| {
        owner
            .issue_root(
                &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                    .expect("the grant log's fence"),
                &principals.operator,
                EvidenceId::new(id).unwrap(),
                agent_principal(DEFAULT_STOCK_AGENT),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap()
    };
    let legacy = super::super::StockGrantSelection {
        operation: issue("legacy-worker"),
        controller: issue("legacy-controller"),
    };
    drop(owner);
    super::super::record_stock_grant_selection(root, "governed", &legacy)
        .await
        .expect("older selection");
    let older = selection(root).await;
    assert_eq!(
        older.agents[DEFAULT_STOCK_AGENT].server_lineage("alpha"),
        Some(&legacy.operation)
    );

    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("first per-target grant");
    assert!(
        alpha
            .cleanup
            .iter()
            .any(|attempt| attempt.grant == legacy.operation),
        "the older worker grant is recorded for cleanup"
    );
    revoke_stock_agent(
        root,
        "governed",
        &principals,
        DEFAULT_STOCK_AGENT,
        &StockGrantSlot::Mcp("alpha".to_string()),
    )
    .await
    .expect("revoke the last target");

    let reopened = selection(root).await;
    let refs = &reopened.agents[DEFAULT_STOCK_AGENT];
    assert_eq!(refs.operation, None);
    for server in ["alpha", "beta"] {
        assert_eq!(refs.server_lineage(server), None, "{server}");
    }
    assert_eq!(
        refs.prompt_selection().map(|(basis, _)| basis),
        Some(WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: Vec::new(),
        }),
        "the empty targeted basis, never the older grant"
    );
    let owner = restore_owner(root, "governed").unwrap();
    assert!(
        !resolves(&owner, &legacy.operation, DEFAULT_STOCK_AGENT),
        "with no unfinished work referencing it, the older grant was retired"
    );
}

/// A selection written before the per-target mode existed can hold both the
/// older worker grant and a per-target grant. Revoking its last per-target
/// grant, before any new issuance, still never selects the older grant: it
/// settles into per-target mode, the older grant is retired, and the prompt
/// basis is the empty targeted one.
#[tokio::test]
async fn revoking_the_last_target_of_an_older_mixed_selection_never_resurrects_it() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let owner = restore_owner(root, "governed").unwrap();
    let issue = |id: &str| {
        owner
            .issue_root(
                &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                    .expect("the grant log's fence"),
                &principals.operator,
                EvidenceId::new(id).unwrap(),
                agent_principal(DEFAULT_STOCK_AGENT),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap()
    };
    let (legacy, controller, alpha) = (issue("legacy"), issue("controller"), issue("alpha"));
    drop(owner);
    let paths = meerkat_store::realm_paths_in(root, "governed");
    let mixed = serde_json::json!({ "agents": { DEFAULT_STOCK_AGENT: {
        "operation": legacy, "controller": controller, "mcp": { "alpha": alpha },
    }}});
    std::fs::write(
        super::super::stock_grants_path(&paths),
        serde_json::to_vec(&mixed).unwrap(),
    )
    .unwrap();
    let before = selection(root).await;
    assert_eq!(
        before.agents[DEFAULT_STOCK_AGENT].server_lineage("beta"),
        None,
        "a mixed selection is read per target"
    );

    let revoked = revoke_stock_agent(
        root,
        "governed",
        &principals,
        DEFAULT_STOCK_AGENT,
        &StockGrantSlot::Mcp("alpha".to_string()),
    )
    .await
    .expect("revoke the last target");
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);

    let after = selection(root).await;
    let refs = &after.agents[DEFAULT_STOCK_AGENT];
    assert!(refs.targeted, "per-target mode is recorded");
    assert_eq!(refs.operation, None);
    for server in ["alpha", "beta"] {
        assert_eq!(refs.server_lineage(server), None, "{server}");
    }
    assert_eq!(
        refs.prompt_selection().map(|(basis, _)| basis),
        Some(WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: Vec::new(),
        })
    );
    let owner = restore_owner(root, "governed").unwrap();
    assert!(!resolves(&owner, &legacy, DEFAULT_STOCK_AGENT), "retired");
}

/// The older selection API can name a grant that is also a per-target
/// selection. Issuing another target retires the older slot but never the
/// grant still selected for its own target.
#[tokio::test]
async fn a_grant_still_selected_elsewhere_is_never_retired() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("alpha")
    .issued;
    let owner = restore_owner(root, "governed").unwrap();
    let controller = owner
        .issue_root(
            &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                .expect("the grant log's fence"),
            &principals.operator,
            EvidenceId::new("controller").unwrap(),
            agent_principal(DEFAULT_STOCK_AGENT),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    drop(owner);
    super::super::record_stock_grant_selection(
        root,
        "governed",
        &super::super::StockGrantSelection {
            operation: alpha.clone(),
            controller,
        },
    )
    .await
    .expect("the older API names alpha's grant");

    let beta = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, mcp("beta", exact(&["read"]))),
    )
    .await
    .expect("beta");
    assert!(
        beta.cleanup.iter().all(|attempt| attempt.grant != alpha),
        "alpha's grant is never queued or retired: {:?}",
        beta.cleanup
    );
    let after = selection(root).await;
    let refs = &after.agents[DEFAULT_STOCK_AGENT];
    assert_eq!(refs.operation, None);
    assert!(!refs.pending_cleanup.contains(&alpha));
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    let owner = restore_owner(root, "governed").unwrap();
    assert!(resolves(&owner, &alpha, DEFAULT_STOCK_AGENT), "still live");
}

/// An MCP server grant records the approval of the server's installation:
/// always as an exact connector domain, and as an exact connector action
/// when its operations are exact. Without the installation nothing is
/// issued.
#[test]
fn an_mcp_grant_records_its_installations_connector_approval() {
    let approval = meerkat_core::McpConnectorApproval::NoConnector;
    let operator = principals().operator;
    let connector_domain = mcp_connector_domain(&operator, "alpha", &approval);
    let connector_action = mcp_connector_action("alpha", &approval);
    let exact_grant = restrictions(&mcp("alpha", exact(&["read"]))).unwrap();
    assert!(lists_domain(
        &exact_grant.resource_domains,
        &connector_domain
    ));
    assert!(lists(&exact_grant.actions, &connector_action));
    // The ordinary Exact grant reads as exactly its raw operation, with its
    // installation entitlement and connector approval kept beside it.
    let view = mcp_operations_view(&exact_grant);
    assert_eq!(
        view.operations,
        StockMcpOperations::Exact(BTreeSet::from(["read".to_string()]))
    );
    assert_eq!(
        view.other_actions,
        BTreeSet::from([action(MCP_INSTALLATION_FEATURE, "alpha"), connector_action])
    );
    assert!(view.unresolved.is_empty());
    let every = restrictions(&mcp("alpha", McpOperations::All)).unwrap();
    assert!(lists_domain(&every.resource_domains, &connector_domain));
    let view = mcp_operations_view(&every);
    assert_eq!(view.operations, StockMcpOperations::All);
    assert!(view.other_actions.is_empty() && view.unresolved.is_empty());
    assert!(
        matches!(
            stock_grant_restrictions(
                &operator,
                &agent("a"),
                &mcp("alpha", exact(&["read"])),
                None,
                None,
                10,
            ),
            Err(StockGovernanceError::ScopeNotRepresentable(_))
        ),
        "no approved installation, no grant"
    );
}

/// Whether an exact bound explicitly lists `value` (a wildcard lists
/// nothing).
fn lists<T: Ord>(restriction: &ExactRestriction<T>, value: &T) -> bool {
    matches!(
        restriction.bound(),
        meerkat_authorization_contracts::constraints::SetBound::Exact(values)
            if values.contains(value)
    )
}

/// Whether an exact domain bound explicitly lists `value` (an unrestricted
/// or owner-scoped bound lists nothing).
fn lists_domain(restriction: &DomainRestriction, value: &ResourceDomain) -> bool {
    matches!(
        restriction.bound(),
        meerkat_authorization_contracts::constraints::DomainBound::Exact(values)
            if values.contains(value)
    )
}

/// The older selection API takes the same administrative custody and keeps
/// per-target mode: recording an older pair for an agent already per target
/// never selects its worker grant, and a concurrent grant is either
/// serialized with it or refused before any effect, never lost.
#[tokio::test]
async fn the_older_selection_api_is_serialized_and_keeps_per_target_mode() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("alpha")
    .issued;
    let owner = restore_owner(root, "governed").unwrap();
    let issue = |id: &str| {
        owner
            .issue_root(
                &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                    .expect("the grant log's fence"),
                &principals.operator,
                EvidenceId::new(id).unwrap(),
                agent_principal(DEFAULT_STOCK_AGENT),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap()
    };
    let older = super::super::StockGrantSelection {
        operation: issue("older-worker"),
        controller: issue("older-controller"),
    };
    drop(owner);

    let beta = request(DEFAULT_STOCK_AGENT, mcp("beta", exact(&["read"])));
    let (recorded, granted) = tokio::join!(
        super::super::record_stock_grant_selection(root, "governed", &older),
        grant_stock_agent(root, "governed", &principals, &beta),
    );
    assert!(
        recorded.is_ok() || matches!(recorded, Err(StockGovernanceError::Persistence(_))),
        "{recorded:?}"
    );
    assert!(
        matches!(granted, Ok(_) | Err(StockGrantError::Refused(_))),
        "{granted:?}"
    );
    let file = selection(root).await;
    let refs = &file.agents[DEFAULT_STOCK_AGENT];
    assert!(refs.targeted, "per-target mode survives the older API");
    assert_eq!(
        refs.operation, None,
        "the older worker grant is never selected"
    );
    assert_eq!(refs.server_lineage("alpha"), Some(&alpha));
    match &granted {
        Ok(outcome) => assert_eq!(refs.server_lineage("beta"), Some(&outcome.issued)),
        Err(_) => assert_eq!(refs.server_lineage("beta"), None),
    }
    if recorded.is_ok() {
        assert_eq!(refs.controller.as_ref(), Some(&older.controller));
    }
}

/// The installation an MCP server grant approves is read and held under the
/// realm config document's writer exclusion until the selection is written:
/// while a config write is in progress the grant is refused before any
/// effect, so a concurrent reconfiguration never interleaves with it.
#[tokio::test]
async fn an_mcp_grant_is_refused_while_the_installation_is_being_changed() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let paths = meerkat_store::realm_paths_in(root, "governed");
    let lock = paths.config_path.with_file_name("config_state.lock");
    let writer = meerkat_core::config_runtime::ConfigWriterLock::try_hold(&lock)
        .unwrap()
        .expect("a config write in progress");
    let before = realm_state(root);
    let outcome = grant_stock_agent(
        root,
        "governed",
        &principals(),
        &request("agent-a", mcp("alpha", exact(&["read"]))),
    )
    .await;
    assert!(
        matches!(
            outcome,
            Err(StockGrantError::Refused(
                StockGovernanceError::ConfigurationUnavailable
            ))
        ),
        "{outcome:?}"
    );
    assert_eq!(realm_state(root), before, "nothing issued or selected");
    assert!(
        meerkat_core::config_runtime::ConfigWriterLock::try_hold(&lock)
            .unwrap()
            .is_none(),
        "the writer's custody is left alone"
    );
    drop(writer);
    grant_stock_agent(
        root,
        "governed",
        &principals(),
        &request("agent-a", mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("granted once the write is done");
    assert!(
        meerkat_core::config_runtime::ConfigWriterLock::try_hold(&lock)
            .unwrap()
            .is_some(),
        "the grant released its own custody once the selection was written"
    );
}

/// A caller that stops waiting never releases the administrative custody
/// while its entered selection write can still land: with the older-API
/// record holding custody inside its write (acknowledged), the caller is
/// cancelled and joined, a later grant is refused while the write is held,
/// and once released the record lands first and the later grant, retried,
/// is the persistent selection beside it.
#[tokio::test]
async fn an_abandoned_selection_write_settles_before_a_later_writer() {
    let tmp = governed_realm().await;
    let root = tmp.path().to_path_buf();
    let principals = principals();
    let owner = restore_owner(&root, "governed").unwrap();
    let issue = |id: &str| {
        owner
            .issue_root(
                &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                    .expect("the grant log's fence"),
                &principals.operator,
                EvidenceId::new(id).unwrap(),
                agent_principal(DEFAULT_STOCK_AGENT),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap()
    };
    let older = super::super::StockGrantSelection {
        operation: issue("older-worker"),
        controller: issue("older-controller"),
    };
    drop(owner);

    let (acknowledge, acknowledged) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let realm_root = meerkat_store::realm_paths_in(&root, "governed").root;
    PUBLICATION_HOLDS
        .lock()
        .unwrap()
        .push((realm_root, acknowledge, released));
    let caller = tokio::spawn({
        let (root, older) = (root.clone(), older.clone());
        async move { super::super::record_stock_grant_selection(&root, "governed", &older).await }
    });
    acknowledged
        .await
        .expect("the record holds custody inside its write");
    caller.abort();
    assert!(
        caller.await.unwrap_err().is_cancelled(),
        "the caller is cancelled and joined"
    );

    let beta = request(DEFAULT_STOCK_AGENT, mcp("beta", exact(&["read"])));
    assert!(
        matches!(
            grant_stock_agent(&root, "governed", &principals, &beta).await,
            Err(StockGrantError::AdmissionInProgress)
        ),
        "a later writer is refused at the realm's fence, which the abandoned write's \
         command keeps with its custody"
    );

    release.send(()).unwrap();
    let mut granted = None;
    for _ in 0..500 {
        match grant_stock_agent(&root, "governed", &principals, &beta).await {
            Ok(outcome) => {
                granted = Some(outcome.issued);
                break;
            }
            Err(StockGrantError::Refused(_) | StockGrantError::AdmissionInProgress) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(other) => panic!("{other:?}"),
        }
    }
    let beta_grant = granted.expect("custody is released once the abandoned write settled");
    let file = selection(&root).await;
    let refs = &file.agents[DEFAULT_STOCK_AGENT];
    assert_eq!(
        refs.controller.as_ref(),
        Some(&older.controller),
        "the abandoned record landed before the later writer"
    );
    assert_eq!(
        refs.server_lineage("beta"),
        Some(&beta_grant),
        "the later writer's selection persists"
    );
}

/// The config runtime of the config document at `document`.
fn config_at(document: &Path) -> meerkat_core::ConfigRuntime {
    meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            document.to_path_buf(),
            meerkat_models::canonical(),
        )),
        document.with_file_name("config_state.json"),
    )
}

fn realm_config(root: &Path) -> meerkat_core::ConfigRuntime {
    config_at(&meerkat_store::realm_paths_in(root, "governed").config_path)
}

/// The installation `server` exactly as the config document at `document`
/// lists it now, at that document's generation: what an operator reviews.
async fn review_at(document: &Path, server: &str) -> ReviewedMcpInstallation {
    let (servers, generation) = meerkat_core::McpRealmPersistTarget::new(&config_at(document))
        .servers()
        .await
        .unwrap();
    ReviewedMcpInstallation {
        server: servers
            .into_iter()
            .find(|installed| installed.name == server)
            .unwrap(),
        generation,
    }
}

/// [`review_at`] the governed fixture realm's own document.
async fn review(root: &Path, server: &str) -> ReviewedMcpInstallation {
    review_at(
        &meerkat_store::realm_paths_in(root, "governed").config_path,
        server,
    )
    .await
}

/// The resolved storage of `realm` in the explicit state root `root`, with
/// the user-global document under `home` when given.
fn storage_for(root: &Path, realm: RealmId, home: Option<&Path>) -> ResolvedStorage {
    ResolvedStorage {
        layout: meerkat_core::storage_layout::StorageLayout::with_injected_roots(
            root.to_path_buf(),
            None,
            home.map(Path::to_path_buf),
            root.to_path_buf(),
        ),
        locator: meerkat_core::runtime_bootstrap::RealmLocator {
            state_root: root.to_path_buf(),
            realm,
        },
        root_choice: meerkat_core::runtime_bootstrap::RealmRootChoice::Explicit,
    }
}

/// The governed fixture realm's storage.
fn governed_storage(root: &Path) -> ResolvedStorage {
    storage_for(root, realm(), None)
}

/// The reserved `global` realm, materialized and governed in `root` by its
/// local operator, whose principals are returned with the realm's own
/// config document: the user-global document under `home`.
async fn governed_global_realm(root: &Path, home: &Path) -> (StockPrincipals, std::path::PathBuf) {
    drop(
        super::super::super::open_governed_realm_persistence_with_provider(
            &crate::DiskStorageProvider,
            root,
            "global",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            super::super::super::RealmGovernance::Ungoverned,
        )
        .await
        .expect("the global realm"),
    );
    let principals = StockPrincipals::for_user(&RealmId::global(), 4242).unwrap();
    super::super::govern_stock_realm(root, "global", &principals)
        .await
        .expect("governed");
    let document = realm_config_source(&storage_for(root, RealmId::global(), Some(home)))
        .unwrap()
        .config_doc_path(&RealmId::global());
    assert_eq!(document, home.join(".rkat").join("config.toml"));
    std::fs::create_dir_all(document.parent().unwrap()).unwrap();
    (principals, document)
}

/// The bytes of the global realm's grant owner and grant selection.
fn global_realm_state(root: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    let paths = meerkat_store::realm_paths_in(root, "global");
    (
        std::fs::read(meerkat_store::grant_owner_store_path(&paths.root)).unwrap(),
        std::fs::read(super::super::stock_grants_path(&paths)).ok(),
    )
}

fn alpha_server() -> meerkat_core::McpServerConfig {
    meerkat_core::McpServerConfig::stdio(
        "alpha",
        "true",
        Vec::new(),
        std::collections::HashMap::new(),
    )
}

/// The reserved `global` realm's installation lives in the user-global
/// document: unchanged there since the review, the reviewed grant is issued
/// and selected.
#[tokio::test]
async fn a_reviewed_global_installation_is_granted_from_the_user_global_document() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = (tmp.path().join("state"), tmp.path().join("home"));
    let (principals, document) = governed_global_realm(&root, &home).await;
    meerkat_core::McpRealmPersistTarget::new(&config_at(&document))
        .add(alpha_server(), None)
        .await
        .unwrap();
    let reviewed = review_at(&document, "alpha").await;
    let granted = grant_stock_agent_for_reviewed_installation(
        &storage_for(&root, RealmId::global(), Some(&home)),
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
        &reviewed,
    )
    .await
    .expect("granted from the user-global document");
    let paths = meerkat_store::realm_paths_in(&root, "global");
    let file = super::super::read_stock_grant_file(&paths)
        .await
        .unwrap()
        .expect("a selection");
    assert_eq!(
        file.agents["agent-a"].server_lineage("alpha"),
        Some(&granted.issued)
    );
}

/// The user-global document changed after the review while a shadow at
/// `<state_root>/global/config.toml` still lists exactly the reviewed
/// installation at the reviewed generation: the grant is refused as changed
/// before any issuance or selection.
#[tokio::test]
async fn a_changed_user_global_document_is_refused_despite_a_matching_shadow() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = (tmp.path().join("state"), tmp.path().join("home"));
    let (principals, document) = governed_global_realm(&root, &home).await;
    let shadow = meerkat_store::realm_paths_in(&root, "global").config_path;
    assert_ne!(shadow, document);
    for listed in [&document, &shadow] {
        meerkat_core::McpRealmPersistTarget::new(&config_at(listed))
            .add(alpha_server(), None)
            .await
            .unwrap();
    }
    let reviewed = review_at(&document, "alpha").await;
    assert_eq!(
        review_at(&shadow, "alpha").await,
        reviewed,
        "the shadow lists exactly the reviewed installation"
    );
    // Another administrator substitutes the actual user-global installation.
    let global = config_at(&document);
    let installations = meerkat_core::McpRealmPersistTarget::new(&global);
    installations.remove("alpha", None).await.unwrap();
    installations
        .add(
            meerkat_core::McpServerConfig::stdio(
                "alpha",
                "substituted",
                Vec::new(),
                std::collections::HashMap::new(),
            ),
            None,
        )
        .await
        .unwrap();
    let before = global_realm_state(&root);
    let outcome = grant_stock_agent_for_reviewed_installation(
        &storage_for(&root, RealmId::global(), Some(&home)),
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
        &reviewed,
    )
    .await;
    assert!(
        matches!(
            &outcome,
            Err(StockGrantError::Refused(StockGovernanceError::InstallationChanged(name)))
                if name == "alpha"
        ),
        "{outcome:?}"
    );
    assert_eq!(
        global_realm_state(&root),
        before,
        "nothing issued or selected"
    );
}

/// A global realm whose layout gives the user-global document no location
/// is refused as unavailable before any effect, never as changed and never
/// from the shadow under the state root (which lists the reviewed
/// installation here).
#[tokio::test]
async fn a_global_review_without_a_user_global_document_is_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = (tmp.path().join("state"), tmp.path().join("home"));
    let (principals, _) = governed_global_realm(&root, &home).await;
    let shadow = meerkat_store::realm_paths_in(&root, "global").config_path;
    meerkat_core::McpRealmPersistTarget::new(&config_at(&shadow))
        .add(alpha_server(), None)
        .await
        .unwrap();
    let reviewed = review_at(&shadow, "alpha").await;
    let before = global_realm_state(&root);
    let outcome = grant_stock_agent_for_reviewed_installation(
        &storage_for(&root, RealmId::global(), None),
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
        &reviewed,
    )
    .await;
    assert!(
        matches!(
            outcome,
            Err(StockGrantError::Refused(
                StockGovernanceError::ConfigurationUnavailable
            ))
        ),
        "{outcome:?}"
    );
    assert_eq!(
        global_realm_state(&root),
        before,
        "nothing issued or selected"
    );
}

/// Milliseconds since the Unix epoch, now.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The critical pair. An unchanged reviewed installation is granted and
/// selected with exactly the requested operations and lifetime, beside the
/// agent's unrelated selection, and the config document is untouched. A
/// descriptor another administrator substituted under the same name after
/// the review is refused as changed, whether the reviewed generation is
/// stale or equal to the current one, with no grant issued and no
/// selection written.
#[tokio::test]
async fn a_reviewed_installation_is_granted_only_while_it_is_unchanged() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let paths = meerkat_store::realm_paths_in(root, "governed");
    let config_bytes = || {
        (
            std::fs::read(&paths.config_path).unwrap(),
            std::fs::read(paths.config_path.with_file_name("config_state.json")).ok(),
        )
    };
    let catalog = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", StockGrantTarget::Catalog),
    )
    .await
    .expect("an unrelated catalog grant")
    .issued;
    let alpha = review(root, "alpha").await;
    let config_before = config_bytes();
    let issued_after = now_ms();
    let granted = grant_stock_agent_for_reviewed_installation(
        &governed_storage(root),
        &principals,
        &StockGrantRequest {
            agent: "agent-a".to_string(),
            target: mcp("alpha", exact(&["read"])),
            expires_in_ms: Some(60_000),
        },
        &alpha,
    )
    .await
    .expect("the unchanged reviewed installation is granted");
    assert!(granted.superseded.is_none());
    assert_eq!(config_bytes(), config_before, "the config is untouched");
    let file = selection(root).await;
    let refs = &file.agents["agent-a"];
    assert_eq!(refs.server_lineage("alpha"), Some(&granted.issued));
    assert_eq!(
        refs.get(&StockGrantSlot::Catalog),
        Some(&catalog),
        "unrelated, preserved"
    );
    let owner = restore_owner(root, "governed").unwrap();
    let resolved = owner
        .resolve_lineage(
            std::slice::from_ref(&granted.issued),
            &agent_principal("agent-a"),
            None,
        )
        .unwrap();
    let operator = principals.operator.clone();
    // The grant's window opens at its own issuance clock, read after
    // `issued_after` and as much later as issuance took: check the bounds
    // at that opening instant.
    let opened_at_ms = resolved.restrictions().lifetime.not_before_ms;
    let covers = |op: &str| {
        resolved
            .restrictions()
            .check_bounds(OperationRestrictionValues {
                action: &action(MCP_FEATURE, op),
                resource_domain: &domain(&operator, "mcp-server/alpha".to_string()),
                processor: &ProcessorRef::Principal {
                    principal: agent_principal("agent-a"),
                },
                audience: &AudienceRef::Principal {
                    principal: operator.clone(),
                },
                now_ms: opened_at_ms,
            })
            .is_ok()
    };
    assert!(covers("read"));
    assert!(!covers("write"));
    let expires_at_ms = resolved.expires_at_ms();
    assert!(
        expires_at_ms >= issued_after + 60_000 && expires_at_ms <= issued_after + 120_000,
        "expires at {expires_at_ms}, reviewed 60000 ms after {issued_after}"
    );
    assert!(resolves(&owner, &catalog, "agent-a"));
    drop(owner);

    // Beta is reviewed as A; another administrator then substitutes B.
    let beta = review(root, "beta").await;
    let config = realm_config(root);
    let installations = meerkat_core::McpRealmPersistTarget::new(&config);
    installations.remove("beta", None).await.unwrap();
    let substituted = meerkat_core::McpServerConfig::stdio(
        "beta",
        "substituted",
        Vec::new(),
        std::collections::HashMap::new(),
    );
    installations.add(substituted, None).await.unwrap();
    let current = review(root, "beta").await;
    assert_ne!(current.server, beta.server);
    let before = realm_state(root);
    for reviewed in [
        beta.clone(),
        ReviewedMcpInstallation {
            generation: current.generation,
            ..beta
        },
    ] {
        let outcome = grant_stock_agent_for_reviewed_installation(
            &governed_storage(root),
            &principals,
            &request("agent-a", mcp("beta", exact(&["read"]))),
            &reviewed,
        )
        .await;
        assert!(
            matches!(
                &outcome,
                Err(StockGrantError::Refused(StockGovernanceError::InstallationChanged(name)))
                    if name == "beta"
            ),
            "{outcome:?}"
        );
    }
    assert_eq!(realm_state(root), before, "no B grant, no selection effect");
}

/// A caller of a reviewed grant that stops waiting while the command holds
/// custody inside its selection write (acknowledged) is cancelled and joined
/// alone: the command keeps custody (a later grant is refused meanwhile),
/// and once the write is released its exact selection lands and its grant
/// resolves.
#[tokio::test]
async fn an_abandoned_reviewed_grant_settles_its_selection() {
    let tmp = governed_realm().await;
    let root = tmp.path().to_path_buf();
    let principals = principals();
    let alpha = review(&root, "alpha").await;
    let (acknowledge, acknowledged) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let realm_root = meerkat_store::realm_paths_in(&root, "governed").root;
    PUBLICATION_HOLDS
        .lock()
        .unwrap()
        .push((realm_root, acknowledge, released));
    let caller = tokio::spawn({
        let (root, principals) = (root.clone(), principals.clone());
        async move {
            grant_stock_agent_for_reviewed_installation(
                &governed_storage(&root),
                &principals,
                &request("agent-a", mcp("alpha", exact(&["read"]))),
                &alpha,
            )
            .await
        }
    });
    acknowledged
        .await
        .expect("the reviewed grant holds custody inside its selection write");
    caller.abort();
    assert!(
        caller.await.unwrap_err().is_cancelled(),
        "only the caller is cancelled and joined"
    );
    let beta = request("agent-a", mcp("beta", exact(&["read"])));
    assert!(
        matches!(
            grant_stock_agent(&root, "governed", &principals, &beta).await,
            Err(StockGrantError::AdmissionInProgress)
        ),
        "the command keeps the realm's fence and its custody while its write is held"
    );
    release.send(()).unwrap();
    // The owned task publishes its selection before it releases the grant
    // owner, so wait for both: the reviewed alpha lineage selected and the
    // exclusive owner restored.
    let mut settled = None;
    let mut last_restore_error = None;
    for _ in 0..500 {
        let selected = selection(&root)
            .await
            .agents
            .get("agent-a")
            .and_then(|refs| refs.server_lineage("alpha").cloned());
        if let Some(selected) = selected {
            match restore_owner(&root, "governed") {
                Ok(owner) => {
                    settled = Some((selected, owner));
                    break;
                }
                Err(error) => last_restore_error = Some(error),
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let Some((selected, owner)) = settled else {
        panic!(
            "the owned task selected the reviewed grant and released the grant owner \
             (last restore error: {last_restore_error:?})"
        );
    };
    assert!(resolves(&owner, &selected, "agent-a"));
}

/// The reviewed descriptor at a moved config generation is not the reviewed
/// installation: refused, with nothing issued or selected.
#[tokio::test]
async fn a_reviewed_installation_at_a_moved_generation_is_refused() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let alpha = review(root, "alpha").await;
    meerkat_core::McpRealmPersistTarget::new(&realm_config(root))
        .add(
            meerkat_core::McpServerConfig::stdio(
                "gamma",
                "true",
                Vec::new(),
                std::collections::HashMap::new(),
            ),
            None,
        )
        .await
        .unwrap();
    let current = review(root, "alpha").await;
    assert_eq!(current.server, alpha.server, "the descriptor is unchanged");
    assert_ne!(current.generation, alpha.generation);
    let before = realm_state(root);
    let outcome = grant_stock_agent_for_reviewed_installation(
        &governed_storage(root),
        &principals(),
        &request("agent-a", mcp("alpha", exact(&["read"]))),
        &alpha,
    )
    .await;
    assert!(
        matches!(
            &outcome,
            Err(StockGrantError::Refused(StockGovernanceError::InstallationChanged(name)))
                if name == "alpha"
        ),
        "{outcome:?}"
    );
    assert_eq!(realm_state(root), before, "nothing issued or selected");
}

/// Only an MCP target naming the reviewed server is a reviewed installation
/// grant: any other target is refused before any effect.
#[tokio::test]
async fn a_reviewed_installation_grant_needs_its_own_mcp_target() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let alpha = review(root, "alpha").await;
    let before = realm_state(root);
    for target in [
        StockGrantTarget::Catalog,
        controller(&[route('1')]),
        bundle_operation("mail", "read_mail"),
        mcp("beta", exact(&["read"])),
    ] {
        let outcome = grant_stock_agent_for_reviewed_installation(
            &governed_storage(root),
            &principals(),
            &request("agent-a", target.clone()),
            &alpha,
        )
        .await;
        assert!(
            matches!(
                outcome,
                Err(StockGrantError::Refused(
                    StockGovernanceError::ScopeNotRepresentable(_)
                ))
            ),
            "{target:?}: {outcome:?}"
        );
    }
    assert_eq!(realm_state(root), before, "nothing issued or selected");
}

#[tokio::test]
async fn an_incomplete_command_task_is_uncertain_never_refused() {
    let outcome: Result<(), StockGrantError> =
        on_owned_task(async { panic!("the command task did not complete") }).await;
    assert!(matches!(outcome, Err(StockGrantError::CommandUncertain)));
    let outcome: Result<(), StockGovernanceError> =
        on_owned_task(async { panic!("the command task did not complete") }).await;
    assert!(matches!(
        outcome,
        Err(StockGovernanceError::CommandUncertain)
    ));
}

/// The offline administrative read of the persisted selection: an ordinary
/// Exact grant reads as its raw operations with its other actions kept, an
/// every-operation grant with a window as `All` and finite, and an
/// unselected slot as not selected. A lineage the selection still names
/// after the owner revoked it reads as not current, with no cause named.
#[tokio::test]
async fn an_observation_reads_the_persisted_selection_through_the_grant_owner() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("alpha");
    let beta = grant_stock_agent(
        root,
        "governed",
        &principals,
        &StockGrantRequest {
            agent: "agent-a".to_string(),
            target: mcp("beta", McpOperations::All),
            expires_in_ms: Some(3_600_000),
        },
    )
    .await
    .expect("beta");
    let slots = [
        StockGrantSlot::Mcp("alpha".to_string()),
        StockGrantSlot::Mcp("beta".to_string()),
        StockGrantSlot::Catalog,
    ];

    let observation = observe_stock_agent_grants(root, "governed", "agent-a", &slots)
        .await
        .expect("observed");
    assert_eq!(observation.realm, realm());
    assert_eq!(observation.agent, "agent-a");
    assert_eq!(observation.executor, agent_principal("agent-a"));
    assert_eq!(
        observation.coverage,
        StockGrantObservationCoverage::PersistedSelection
    );
    let [alpha_slot, beta_slot, catalog_slot] = observation.slots.as_slice() else {
        panic!("one entry per requested slot: {observation:?}");
    };
    assert_eq!(alpha_slot.slot, slots[0]);
    let alpha_selected = alpha_slot.selected.as_ref().expect("alpha is selected");
    assert_eq!(alpha_selected.lineage, alpha.issued);
    assert_eq!(alpha_selected.source, StockSelectionSource::PerTarget);
    let StockGrantConclusion::Current(alpha_facts) = &alpha_selected.conclusion else {
        panic!("alpha is current: {alpha_selected:?}");
    };
    assert_eq!(alpha_facts.lifetime, StockGrantLifetime::Unrestricted);
    assert_eq!(alpha_facts.lineage_restrictions.len(), 1);
    let alpha_operations = alpha_facts.mcp_operations.as_ref().expect("an MCP slot");
    assert_eq!(
        alpha_operations.operations,
        StockMcpOperations::Exact(BTreeSet::from(["read".to_string()]))
    );
    assert_eq!(
        alpha_operations.other_actions,
        BTreeSet::from([
            action(MCP_INSTALLATION_FEATURE, "alpha"),
            mcp_connector_action("alpha", &meerkat_core::McpConnectorApproval::NoConnector),
        ])
    );
    let beta_selected = beta_slot.selected.as_ref().expect("beta is selected");
    let StockGrantConclusion::Current(beta_facts) = &beta_selected.conclusion else {
        panic!("beta is current: {beta_selected:?}");
    };
    assert!(
        matches!(
            beta_facts.lifetime,
            StockGrantLifetime::Finite { expires_at_ms } if expires_at_ms < u64::MAX
        ),
        "a window is finite: {:?}",
        beta_facts.lifetime
    );
    let beta_operations = beta_facts.mcp_operations.as_ref().expect("an MCP slot");
    assert_eq!(beta_operations.operations, StockMcpOperations::All);
    assert!(
        catalog_slot.selected.is_none(),
        "no catalog grant is selected"
    );

    // The owner revokes beta while the selection still names it.
    let bundle = administrative_custody(root, "governed")
        .await
        .expect("custody");
    let owner = restore_owner(root, "governed").unwrap();
    let machine = bundle.runtime_adapter();
    let mut custody = machine
        .try_controller_grant_mutation()
        .expect("the realm's grant custody");
    owner
        .revoke(
            &meerkat_core::AdmissionGuard::try_acquire(owner.admission_fences())
                .expect("the grant log's fence"),
            &principals.operator,
            &beta.issued,
            &mut custody,
        )
        .expect("revoked, still selected");
    drop(custody);
    drop(machine);
    drop(owner);
    drop(bundle);

    let observation = observe_stock_agent_grants(root, "governed", "agent-a", &slots)
        .await
        .expect("observed again");
    let beta_selected = observation.slots[1]
        .selected
        .as_ref()
        .expect("still selected");
    assert_eq!(beta_selected.lineage, beta.issued);
    assert_eq!(beta_selected.conclusion, StockGrantConclusion::NotCurrent);
    assert!(matches!(
        observation.slots[0]
            .selected
            .as_ref()
            .map(|selected| &selected.conclusion),
        Some(StockGrantConclusion::Current(_))
    ));
}

/// A served realm observes its retained selection through its own
/// composition's grant owner, the lineage its prompts use, with nothing
/// reopened or restored, while the offline read is refused. Before the realm
/// opened and after it closed, no composition holds that owner and the
/// retained read is refused: the read keeps no owner alive. Reopened by a
/// governance with a distinct supplied executor, the live read reports and
/// resolves for that executor, the one its composition uses.
#[tokio::test]
async fn an_open_realm_observes_its_retained_selection_through_its_own_owner() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("agent-a", mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("alpha");
    let default_alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request(DEFAULT_STOCK_AGENT, mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("the default agent's alpha");
    let slots = [
        StockGrantSlot::Mcp("alpha".to_string()),
        StockGrantSlot::Mcp("beta".to_string()),
    ];
    let governance =
        super::super::StockGovernance::new_for_agent(realm(), principals.clone(), "agent-a")
            .unwrap();
    assert!(
        matches!(
            governance.observe_retained_grants(&slots),
            Err(StockGovernanceError::GrantOwner(_))
        ),
        "no composition yet"
    );
    let (_, bundle) = super::super::super::open_governed_realm_persistence_with_provider(
        &crate::DiskStorageProvider,
        root,
        "governed",
        None,
        None,
        None,
        super::super::super::RealmGovernance::Stock(Arc::clone(&governance)),
    )
    .await
    .expect("the realm opens with its stock composition");

    let observation = governance
        .observe_retained_grants(&slots)
        .expect("observed through the open composition");
    assert_eq!(
        observation.coverage,
        StockGrantObservationCoverage::RetainedSelection
    );
    assert_eq!(observation.agent, "agent-a");
    assert_eq!(observation.executor, agent_principal("agent-a"));
    let alpha_selected = observation.slots[0]
        .selected
        .as_ref()
        .expect("alpha is retained");
    assert_eq!(alpha_selected.lineage, alpha.issued);
    assert!(matches!(
        alpha_selected.conclusion,
        StockGrantConclusion::Current(_)
    ));
    assert!(observation.slots[1].selected.is_none());
    assert!(
        observe_stock_agent_grants(root, "governed", "agent-a", &slots)
            .await
            .is_err(),
        "the offline read is refused while the realm is served"
    );

    drop(bundle);
    let mut closed = false;
    for _ in 0..500 {
        if matches!(
            governance.observe_retained_grants(&slots),
            Err(StockGovernanceError::GrantOwner(_))
        ) {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        closed,
        "a closed realm's owner is not kept alive by the read"
    );

    // A governance built with a supplied executor that is not its agent's
    // derived principal: the live read reports and resolves for the executor
    // the composition uses, so the default agent's selected grant, issued to
    // that agent's own executor, is not current for it.
    let mut supplied = principals;
    supplied.executor = agent_principal("agent-b");
    let live = super::super::StockGovernance::new(realm(), supplied);
    let mut reopened = None;
    for _ in 0..500 {
        if let Ok(opened) = super::super::super::open_governed_realm_persistence_with_provider(
            &crate::DiskStorageProvider,
            root,
            "governed",
            None,
            None,
            None,
            super::super::super::RealmGovernance::Stock(Arc::clone(&live)),
        )
        .await
        {
            reopened = Some(opened);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (_, bundle) = reopened.expect("the realm reopens once the first host released it");
    let observation = live
        .observe_retained_grants(&slots)
        .expect("observed through the reopened composition");
    assert_eq!(observation.agent, DEFAULT_STOCK_AGENT);
    assert_eq!(
        observation.executor,
        agent_principal("agent-b"),
        "the executor the composition uses, not the agent's derived one"
    );
    let default_selected = observation.slots[0]
        .selected
        .as_ref()
        .expect("the default agent's alpha is retained");
    assert_eq!(default_selected.lineage, default_alpha.issued);
    assert_eq!(
        default_selected.conclusion,
        StockGrantConclusion::NotCurrent,
        "the owner refuses that lineage for the composition's executor"
    );
    drop(bundle);
}

// ---------------------------------------------------------------------------
// The realm's admission fence: every stock command takes it first.
// ---------------------------------------------------------------------------

/// R of the governed fixture realm under `root`.
fn fixture_fence(root: &Path) -> meerkat_core::AdmissionFence {
    meerkat_core::AdmissionFence::realm(root, &realm()).expect("the realm's fence")
}

/// With no holder, a reviewed MCP installation's grant is issued and
/// selected through the realm's fence, which the command releases when it
/// settles: the fence is free again at once.
#[tokio::test]
async fn with_no_holder_reviewed_mcp_grant_issuance_succeeds() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let reviewed = review(root, "alpha").await;
    let outcome = grant_stock_agent_for_reviewed_installation(
        &governed_storage(root),
        &principals(),
        &request("a", mcp("alpha", exact(&["read"]))),
        &reviewed,
    )
    .await
    .expect("issued and selected with no holder");
    let selected = selection(root).await;
    assert_eq!(
        selected
            .agents
            .get("a")
            .and_then(|refs| refs.get(&StockGrantSlot::Mcp("alpha".into()))),
        Some(&outcome.issued)
    );
    meerkat_core::AdmissionGuard::try_acquire(vec![fixture_fence(root)])
        .expect("the command released the realm's fence");
}

/// An admission's hold of the realm's fence (SHARED, as an admission holds
/// it) refuses every stock grant command once its bound has passed, before
/// any custody, typed: nothing is issued, selected or revoked. Once the
/// hold ends the same commands run.
#[tokio::test]
async fn under_a_held_fence_issue_revoke_and_select_refuse_not_started() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    let alpha = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("a", mcp("alpha", exact(&["read"]))),
    )
    .await
    .expect("granted with no holder");
    let before = realm_state(root);
    let mut hold = meerkat_core::AdmissionHold::try_acquire(vec![fixture_fence(root)])
        .expect("an admission holds the realm's fence");

    let started = std::time::Instant::now();
    assert!(matches!(
        grant_stock_agent(
            root,
            "governed",
            &principals,
            &request("a", mcp("beta", exact(&["read"]))),
        )
        .await,
        Err(StockGrantError::AdmissionInProgress)
    ));
    assert!(
        started.elapsed() >= meerkat_core::realm_admission::INTERACTIVE_FENCE_WAIT,
        "the grant waited its bound before it refused"
    );
    let reviewed = review(root, "beta").await;
    assert!(matches!(
        grant_stock_agent_for_reviewed_installation(
            &governed_storage(root),
            &principals,
            &request("a", mcp("beta", exact(&["read"]))),
            &reviewed,
        )
        .await,
        Err(StockGrantError::AdmissionInProgress)
    ));
    assert!(matches!(
        revoke_stock_agent(
            root,
            "governed",
            &principals,
            "a",
            &StockGrantSlot::Mcp("alpha".into()),
        )
        .await,
        Err(StockGovernanceError::AdmissionInProgress)
    ));
    assert_eq!(
        realm_state(root),
        before,
        "nothing was issued, selected or revoked"
    );

    hold.release();
    let revoked = revoke_stock_agent(
        root,
        "governed",
        &principals,
        "a",
        &StockGrantSlot::Mcp("alpha".into()),
    )
    .await
    .expect("revoked once the hold ended");
    assert_eq!(revoked.revoked, alpha.issued);
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
}

/// Recording the operator's selection takes the realm's fence first: under
/// an admission's hold it refuses, typed, with the selection unchanged, and
/// records once the hold ends.
#[tokio::test]
async fn record_stock_grant_selection_refuses_under_a_held_fence() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let principals = principals();
    // Two of the owner's grants, issued for another agent, recorded as the
    // default agent's older pair (the selection names references only).
    let operation = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("other", StockGrantTarget::Catalog),
    )
    .await
    .expect("a catalog grant")
    .issued;
    let controller = grant_stock_agent(
        root,
        "governed",
        &principals,
        &request("other", controller(&[route('a')])),
    )
    .await
    .expect("a controller grant")
    .issued;
    let recorded = super::super::StockGrantSelection {
        operation,
        controller,
    };
    let before = realm_state(root);
    let mut hold = meerkat_core::AdmissionHold::try_acquire(vec![fixture_fence(root)])
        .expect("an admission holds the realm's fence");
    assert!(matches!(
        super::super::record_stock_grant_selection(root, "governed", &recorded).await,
        Err(StockGovernanceError::AdmissionInProgress)
    ));
    assert_eq!(realm_state(root), before, "the selection is unchanged");

    hold.release();
    super::super::record_stock_grant_selection(root, "governed", &recorded)
        .await
        .expect("recorded once the hold ended");
    let refs = selection(root).await;
    let refs = refs.agents.get(DEFAULT_STOCK_AGENT).expect("the agent");
    assert_eq!(refs.operation.as_ref(), Some(&recorded.operation));
    assert_eq!(refs.controller.as_ref(), Some(&recorded.controller));
}
