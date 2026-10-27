//! The checked MCP route's native isolation control: two sessions' actual
//! adapters on the real MCP fixture server, the stock policy over a real
//! grant owner, and the native fenced dispatch that prepares each
//! operation's authorization binding and checks it again at the reviewed
//! entry.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, ResourceDomain};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate, OriginalWorkRef,
    QualifiedIngressNamespace, TargetGrantLineage, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
    OperationRefusalKind, PreparedAuthorizationBinding, ToolAuthorizationFacts,
    ToolAuthorizationTarget,
};
use meerkat_core::connection::RealmId;
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::types::{SessionId, ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, McpServerConfig, PrincipalKind, PrincipalRef, ToolDispatchContext,
    ToolError, ToolExecutionResolutionError, ToolOperationTarget, TrustDomainId,
};
use meerkat_mcp::{CheckedMcpRoute, CheckedMcpRouteSpec, McpRouter, McpRouterAdapter};

use super::StockPrincipals;
use super::destination::StockSessionDestination;
use super::issuance::{McpOperations, StockGrantTarget, stock_grant_restrictions};
use super::policy::StockInvocationPolicy;

const LIMIT: Duration = Duration::from_secs(10);
const SERVER: &str = "setup";
const ROUTE: &str = "checked-setup";
/// The name the server's configuration exposes its `add` under.
const EXPOSED: &str = "plus";

fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, id, TrustDomainId::new("checked-route").unwrap()).unwrap()
}

fn operator() -> PrincipalRef {
    principal(PrincipalKind::Human, "operator")
}

fn ingress() -> PrincipalRef {
    principal(PrincipalKind::RuntimeHost, "rkat-cli")
}

fn executor() -> PrincipalRef {
    principal(PrincipalKind::PersonalAgent, "executor")
}

fn realm() -> RealmId {
    RealmId::parse("checked-realm").unwrap()
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: ingress(),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([7; 32]),
    }
}

/// The realm's installations, for attachment: not consulted by tool calls.
struct AnyInstallation;
impl ConfiguredMcpAttachmentAuthority for AnyInstallation {
    fn authorize_configured(&self, _: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        Ok(())
    }
}

/// No controller work holds these grants.
struct NoController;
impl ControllerGrantMutationCustody for NoController {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

fn grant_owner() -> Arc<LocalGrantAuthority> {
    Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: operator(),
                namespace: id("stock-grants"),
                generation: 1,
            },
            meerkat_authorization::publication::LocalAuthorizationPublication::new(),
            Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
        )
        .unwrap(),
    )
}

/// The operator's stock grant of `target` to the executor, recording the
/// fixture installation's connector approval (it has no connector binding).
fn issue(grants: &LocalGrantAuthority, name: &str, target: &StockGrantTarget) -> GrantLineageRef {
    let restrictions = stock_grant_restrictions(
        &operator(),
        &executor(),
        target,
        Some(&meerkat_core::McpConnectorApproval::NoConnector),
        None,
        0,
    )
    .unwrap();
    grants
        .issue_root(&operator(), id(name), executor(), None, restrictions)
        .unwrap()
}

fn server_target() -> GrantTarget {
    GrantTarget::McpServer {
        server: SERVER.into(),
    }
}

fn route_target() -> GrantTarget {
    GrantTarget::CheckedMcpRoute {
        route: ROUTE.into(),
        server: SERVER.into(),
    }
}

/// The operator's own stock work at `session`, selecting exactly `targets`.
fn association(
    session: &SessionId,
    targets: Vec<(GrantTarget, GrantLineageRef)>,
) -> InputAuthorityAssociation {
    let mut targets: Vec<TargetGrantLineage> = targets
        .into_iter()
        .map(|(target, lineage)| TargetGrantLineage {
            target,
            lineage: vec![lineage],
        })
        .collect();
    targets.sort_by(|left, right| left.target.cmp(&right.target));
    let controller = targets[0].lineage.clone();
    let target = StockSessionDestination::for_session(&operator(), session, 1)
        .unwrap()
        .target()
        .clone();
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: operator(),
        ingress_actor: ingress(),
        represented_subject: None,
        original_authentication: evidence("authentication"),
        logical_executor: executor(),
        target,
        original_work: OriginalWorkRef {
            authority: ingress(),
            work: id("work-1"),
        },
        root_event: evidence("event"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets,
        },
        controller_grant_lineage: controller,
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: realm(),
            ingress: ResourceDomain {
                authority: ingress(),
                namespace: "cli-prompts".into(),
            },
            occurrence_scope: id("occurrence-1"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .unwrap()
}

/// The session's governed dispatch context over `policy`'s actual work
/// authorization for `association`.
fn governed(
    policy: &Arc<GrantBackedWorkPolicy>,
    session: &SessionId,
    association: InputAuthorityAssociation,
) -> ToolDispatchContext {
    let work = policy
        .work_context(Arc::from([association]), OperationExecutionScope::Domain)
        .unwrap();
    ToolDispatchContext::default()
        .with_runtime_identity(session.clone(), None)
        .with_work_authorization(Some(work))
}

/// The fixture server installed with a ceiling of `echo`: `add` is outside
/// it, and its configuration exposes `add` as `plus`.
fn server_config() -> McpServerConfig {
    let mut server = McpServerConfig::stdio(
        SERVER,
        mcp_test_server::fixture_binary()
            .to_string_lossy()
            .to_string(),
        Vec::new(),
        HashMap::new(),
    );
    server.access = Some(meerkat_core::McpServerAccess::allowing(["echo"]));
    server.tool_names.insert("add".into(), EXPOSED.into());
    server
}

/// One session's own adapter over its own connection to the fixture.
async fn adapter() -> Arc<McpRouterAdapter> {
    let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
        Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    );
    tokio::time::timeout(LIMIT, router.add_server(server_config()))
        .await
        .unwrap()
        .unwrap();
    Arc::new(McpRouterAdapter::new(router))
}

fn spec() -> CheckedMcpRouteSpec {
    CheckedMcpRouteSpec {
        route: ROUTE.into(),
        server: SERVER.into(),
        operations: ["add".to_string()].into(),
    }
}

/// The session's root: its adapter and its checked route, composed.
fn root(
    adapter: &Arc<McpRouterAdapter>,
    route: &Arc<CheckedMcpRoute>,
) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(meerkat_core::DynamicToolComposite::new(vec![
        Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>,
        Arc::clone(route) as Arc<dyn AgentToolDispatcher>,
    ]))
}

fn resolve(
    root: &Arc<dyn AgentToolDispatcher>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
) -> Result<meerkat_core::ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
    meerkat_core::agent::resolve_tool_execution_plan_fenced(
        root,
        call,
        context,
        &meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_mins(10),
                ),
            ])
            .unwrap(),
        ),
    )
}

async fn dispatch(
    root: &Arc<dyn AgentToolDispatcher>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &meerkat_core::ResolvedToolExecutionPlan,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    meerkat_core::agent::dispatch_tool_execution_plan_fenced(root, call, context, plan).await
}

fn names(tools: &[Arc<ToolDef>]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

fn is_denied(error: &ToolError) -> bool {
    matches!(error, ToolError::AuthorizationRefused { refusal }
        if refusal.kind() == OperationRefusalKind::Denied)
}

/// The same raw operation as a binding whose single owner target is the
/// server's ordinary operation, as a raw route would produce it.
fn ordinary_binding(raw_operation: &str) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(ToolDef {
            name: raw_operation.into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
            audience: Default::default(),
        }),
        true,
    );
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_mins(10),
                ),
            ])
            .unwrap(),
        )
        .unwrap()
        .with_owner_witness(
            meerkat_core::ToolExecutionOwnerWitness::new(
                "raw-route",
                raw_operation,
                meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
            )
            .unwrap()
            .with_operation_target(ToolOperationTarget::McpServerOperation {
                server: SERVER.into(),
                connection_generation: 1,
                raw_operation: raw_operation.into(),
                connector: meerkat_core::McpConnectorApproval::NoConnector,
                account: Arc::new(meerkat_core::OperationAccount::Unavailable),
            }),
        )
        .unwrap();
    PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: meerkat_core::OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
            call_id: Arc::from("call-raw"),
            name: raw_operation.into(),
            arguments: serde_json::value::RawValue::from_string("{}".to_string())
                .unwrap()
                .into(),
            target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
        }),
    })
}

/// Two sessions, each with its own adapter and its own checked route to the
/// same installed server, under one realm's grant owner. Session A's work
/// selects the server's ordinary grant (echo) and the route's own grant
/// (add); session B's selects only the ordinary grant.
#[tokio::test]
async fn a_checked_route_needs_its_own_grant_its_own_session_and_an_open_route() {
    let grants = grant_owner();
    let principals = StockPrincipals {
        operator: operator(),
        ingress_actor: ingress(),
        executor: executor(),
    };
    let stock = StockInvocationPolicy::new(
        Arc::clone(&grants),
        &principals,
        realm(),
        Arc::new(AnyInstallation),
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        Arc::clone(&grants),
        stock.clone(),
        stock,
    ));
    let ordinary = issue(
        &grants,
        "setup-ordinary",
        &StockGrantTarget::Mcp {
            server: SERVER.into(),
            operations: McpOperations::Exact(["echo".to_string()].into()),
        },
    );
    let checked = issue(
        &grants,
        "setup-checked",
        &StockGrantTarget::CheckedMcpRoute {
            route: ROUTE.into(),
            server: SERVER.into(),
            operations: ["add".to_string()].into(),
        },
    );

    let session_a = SessionId::new();
    let session_b = SessionId::new();
    let context_a = governed(
        &policy,
        &session_a,
        association(
            &session_a,
            vec![
                (server_target(), ordinary.clone()),
                (route_target(), checked.clone()),
            ],
        ),
    );
    let context_b = governed(
        &policy,
        &session_b,
        association(&session_b, vec![(server_target(), ordinary)]),
    );
    let adapter_a = adapter().await;
    let adapter_b = adapter().await;
    let route_a = adapter_a.checked_route(session_a.clone(), spec()).unwrap();
    let route_b = adapter_b.checked_route(session_b.clone(), spec()).unwrap();
    let root_a = root(&adapter_a, &route_a);
    let root_b = root(&adapter_b, &route_b);
    let add_args = serde_json::value::RawValue::from_string(r#"{"a":1,"b":2}"#.into()).unwrap();
    let echo_args = serde_json::value::RawValue::from_string(r#"{"message":"hi"}"#.into()).unwrap();
    let plus = ToolCallView {
        id: "call-add",
        name: EXPOSED,
        args: &add_args,
    };
    let echo = ToolCallView {
        id: "call-echo",
        name: "echo",
        args: &echo_args,
    };

    // The ordinary projection keeps the ceiling; the route offers exactly
    // its operation, under the configured name.
    assert_eq!(names(&adapter_a.tools()), vec!["echo"]);
    assert_eq!(names(&route_a.tools()), vec![EXPOSED]);

    // A matching runtime identity on an ungoverned (default) context is not
    // bound work: refused before entry.
    let ungoverned = ToolDispatchContext::default().with_runtime_identity(session_a.clone(), None);
    let plan = resolve(&root_a, plus, &ungoverned).expect("ungoverned plan");
    assert_eq!(
        dispatch(&root_a, plus, &ungoverned, &plan)
            .await
            .unwrap_err(),
        ToolError::access_denied(EXPOSED)
    );

    // Session A: the checked route, its own grant and its own session.
    let plan_a = resolve(&root_a, plus, &context_a).expect("A resolves the checked route");
    let outcome = dispatch(&root_a, plus, &context_a, &plan_a)
        .await
        .expect("the checked route is allowed by its own grant");
    assert_eq!(outcome.result.text_content(), "3");
    // The identical replay is refused natively. The same call id resolved
    // afresh, as a provider can repeat it in a later turn, is a new
    // occurrence: it enters once, under its own prepared authorization.
    let changed = ToolError::unavailable(
        EXPOSED,
        meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
    );
    assert_eq!(
        dispatch(&root_a, plus, &context_a, &plan_a)
            .await
            .unwrap_err(),
        changed
    );
    let later = resolve(&root_a, plus, &context_a).expect("a later turn's plan");
    let outcome = dispatch(&root_a, plus, &context_a, &later)
        .await
        .expect("a new occurrence enters");
    assert_eq!(outcome.result.text_content(), "3");
    assert_eq!(
        dispatch(&root_a, plus, &context_a, &later)
            .await
            .unwrap_err(),
        changed
    );

    // The identical raw operation: the projection never routes it, by its
    // raw name or its configured name, the session's root offers it only
    // through the route, and the server's own lineage refuses it as an
    // ordinary target.
    let raw: Arc<dyn AgentToolDispatcher> = adapter_a.clone();
    let raw_add = ToolCallView {
        id: "call-add-raw",
        name: "add",
        args: &add_args,
    };
    for call in [raw_add, plus] {
        assert!(matches!(
            resolve(&raw, call, &context_a),
            Err(ToolExecutionResolutionError::NotFound { .. })
        ));
    }
    assert!(matches!(
        resolve(&root_a, raw_add, &context_a),
        Err(ToolExecutionResolutionError::NotFound { .. })
    ));
    let work_a = context_a.work_authorization().unwrap();
    let ordinary_refusal = work_a
        .authorization()
        .prepare(&ordinary_binding("add"))
        .map(|_| ())
        .unwrap_err();
    assert!(
        matches!(&ordinary_refusal, OperationAuthorizationError::Refused(refusal)
            if refusal.kind() == OperationRefusalKind::Denied),
        "{ordinary_refusal:?}"
    );

    // Session B: its own route, but its work selects no lineage for the
    // route, so the identical call is denied at native authorization.
    let plan_b = resolve(&root_b, plus, &context_b).expect("B resolves its own route");
    let refused = dispatch(&root_b, plus, &context_b, &plan_b)
        .await
        .unwrap_err();
    assert!(is_denied(&refused), "{refused:?}");
    // B never runs A's plan, through its own root or through A's.
    assert!(dispatch(&root_b, plus, &context_b, &plan_a).await.is_err());
    assert!(dispatch(&root_a, plus, &context_b, &plan_a).await.is_err());

    // The admitted sibling works for both sessions.
    for (root, context) in [(&root_a, &context_a), (&root_b, &context_b)] {
        let plan = resolve(root, echo, context).expect("echo plan");
        let outcome = dispatch(root, echo, context, &plan)
            .await
            .expect("echo is allowed by the ordinary grant");
        assert_eq!(outcome.result.text_content(), "hi");
    }

    // After A closes its route nothing resolves, and a plan resolved before
    // the close is refused, while its grant is still current.
    let unentered = ToolCallView {
        id: "call-add-unentered",
        name: EXPOSED,
        args: &add_args,
    };
    let plan = resolve(&root_a, unentered, &context_a).expect("plan before the close");
    route_a.close();
    assert!(route_a.tools().is_empty());
    assert!(matches!(
        resolve(&root_a, plus, &context_a),
        Err(ToolExecutionResolutionError::NotFound { .. })
    ));
    assert!(
        dispatch(&root_a, unentered, &context_a, &plan)
            .await
            .is_err()
    );

    // A new registration of the route serves A again, until the operator
    // revokes the route's grant: then the call is denied, and the ordinary
    // sibling is untouched.
    let reopened = adapter_a.checked_route(session_a.clone(), spec()).unwrap();
    let root_again = root(&adapter_a, &reopened);
    let again = ToolCallView {
        id: "call-add-again",
        name: EXPOSED,
        args: &add_args,
    };
    let plan = resolve(&root_again, again, &context_a).expect("reopened plan");
    dispatch(&root_again, again, &context_a, &plan)
        .await
        .expect("the reopened route is allowed");
    grants
        .revoke(&operator(), &checked, &mut NoController)
        .expect("revoke the route's grant");
    let after = ToolCallView {
        id: "call-add-after-revocation",
        name: EXPOSED,
        args: &add_args,
    };
    let plan = resolve(&root_again, after, &context_a).expect("plan after revocation");
    let revoked = dispatch(&root_again, after, &context_a, &plan)
        .await
        .unwrap_err();
    assert!(is_denied(&revoked), "{revoked:?}");
    let plan = resolve(&root_again, echo, &context_a).expect("echo plan");
    dispatch(&root_again, echo, &context_a, &plan)
        .await
        .expect("the ordinary sibling is untouched");

    adapter_a.shutdown().await;
    adapter_b.shutdown().await;
}
