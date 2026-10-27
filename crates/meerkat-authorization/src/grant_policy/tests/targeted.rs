//! Exact grant lineages selected per typed operation target: each MCP server's
//! operations resolve only the lineage selected for that server, contributing
//! originals stay conjunctive and a single lineage keeps its meaning.
use super::*;
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
use meerkat_core::authorization::{ToolAuthorizationFacts, ToolAuthorizationTarget};
use meerkat_core::tool_execution::ToolOperationTarget;

/// Admits every original of the fixture's exact execution scope.
struct AnyAdmitted;
impl AdmittedWorkPolicyOwner for AnyAdmitted {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if binding.facts().execution_scope != scope() {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 1_000,
        })
    }
}

/// Maps a dispatched MCP operation to its raw operation on the server's own
/// domain, a tool catalog control to `catalog-read` on `tool-catalog`, a
/// registered bundle operation to that operation on the bundle's own domain,
/// and an external source read to `read` on its namespace.
struct McpOperations;
impl OperationPolicyOwner for McpOperations {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let values = match &binding.facts().operation {
            AuthorizationOperation::Tool(ToolAuthorizationFacts {
                target: ToolAuthorizationTarget::Dispatcher(plan),
                ..
            }) => match single_tool_operation_target(plan)? {
                Some(
                    ToolOperationTarget::McpServerOperation {
                        server,
                        raw_operation,
                        ..
                    }
                    | ToolOperationTarget::CheckedMcpServerOperation {
                        server,
                        raw_operation,
                        ..
                    },
                ) => tuple(raw_operation, &format!("mcp-{server}")),
                Some(ToolOperationTarget::CatalogControl { .. }) => {
                    tuple("catalog-read", "tool-catalog")
                }
                Some(ToolOperationTarget::RegisteredToolBundleOperation {
                    bundle,
                    operation,
                    ..
                }) => tuple(operation, &format!("bundle-{bundle}")),
                // One delivery to one peer: the delivery kind on that peer.
                Some(ToolOperationTarget::PeerSend { peer, kind }) => {
                    let action = match kind {
                        meerkat_core::PeerSendKind::Message => "send-message",
                        meerkat_core::PeerSendKind::Request => "send-request",
                        _ => return Err(denied().into()),
                    };
                    tuple(action, &format!("peer-{peer}"))
                }
                Some(ToolOperationTarget::PeerDirectoryRead) => tuple("read", "peer-directory"),
                // A read of one session's memory, on that session's memory.
                Some(ToolOperationTarget::MemoryOperation {
                    scope: meerkat_core::MemoryScopeRef::Session { session_id },
                    operation: meerkat_core::MemoryOperation::Read,
                }) => tuple("memory-read", &format!("memory-{session_id}")),
                _ => return Err(denied().into()),
            },
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                usage: SourceAuthorizationUse::Read,
                target: SourceAuthorizationTarget::External(target),
            }) => tuple("read", &target.namespace),
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![values],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// No unfinished controller work holds these grants.
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

fn mcp_call(targets: &[(&str, &str)]) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(meerkat_core::ToolDef {
            name: "mcp_tool".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
            audience: Default::default(),
        }),
        true,
    );
    let mut plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        )
        .expect("plan");
    for (index, (server, raw)) in targets.iter().enumerate() {
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            format!("execution-owner-{index}"),
            "mcp_tool",
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
        )
        .expect("owner witness")
        .with_operation_target(ToolOperationTarget::McpServerOperation {
            server: (*server).to_string(),
            connection_generation: 1,
            raw_operation: (*raw).to_string(),
            connector: meerkat_core::McpConnectorApproval::NoConnector,
            account: std::sync::Arc::new(meerkat_core::OperationAccount::Unavailable),
        });
        plan = plan.with_owner_witness(witness).expect("witness");
    }
    bind(AuthorizationOperation::Tool(ToolAuthorizationFacts {
        call_id: Arc::from("call-1"),
        name: "mcp_tool".into(),
        arguments: serde_json::value::RawValue::from_string("{}".to_string())
            .expect("raw arguments")
            .into(),
        target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
    }))
}

/// A dispatched call whose single owner witness is the session's tool
/// catalog control for `operation`.
fn catalog_call(
    operation: meerkat_core::tool_execution::CatalogOperation,
) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(meerkat_core::ToolDef {
            name: "tool_catalog".into(),
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
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        )
        .expect("plan")
        .with_owner_witness(
            meerkat_core::ToolExecutionOwnerWitness::new(
                "catalog-owner",
                "tool_catalog",
                meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
            )
            .expect("owner witness")
            .with_operation_target(ToolOperationTarget::CatalogControl { operation }),
        )
        .expect("witness");
    bind(AuthorizationOperation::Tool(ToolAuthorizationFacts {
        call_id: Arc::from("call-catalog"),
        name: "tool_catalog".into(),
        arguments: serde_json::value::RawValue::from_string("{}".to_string())
            .expect("raw arguments")
            .into(),
        target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
    }))
}

fn server(name: &str) -> GrantTarget {
    GrantTarget::McpServer {
        server: name.into(),
    }
}

impl Fixture {
    /// A root grant to the executor for `actions` (all when empty) on the
    /// listed servers' domains only.
    fn server_grant(
        &self,
        id_name: &str,
        actions: &[&str],
        servers: &[&str],
        lifetime: LifetimeRestriction,
    ) -> GrantLineageRef {
        let actions = if actions.is_empty() {
            ExecutionRestrictions::unrestricted().actions
        } else {
            ExactRestriction::exact(actions.iter().map(|name| action(name)))
        };
        self.grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions {
                    actions,
                    resource_domains: DomainRestriction::exact(
                        servers.iter().map(|name| domain(&format!("mcp-{name}"))),
                    ),
                    lifetime,
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("server grant")
    }

    /// The fixture's original, with `basis` and an unrestricted admitted
    /// ceiling (so only grants bound the operations below).
    fn original_with(&self, work: &str, basis: WorkAuthorityBasis) -> InputAuthorityAssociation {
        let mut candidate = self.association().candidate().clone();
        candidate.authority_basis = basis;
        candidate.admitted_ceiling = ExecutionRestrictions::unrestricted();
        candidate.original_work.work = id(work);
        InputAuthorityAssociation::new(candidate).expect("association")
    }

    /// A targeted selection with the fixture's chain as the optional base.
    fn targeted(&self, targets: Vec<(&str, GrantLineageRef)>) -> WorkAuthorityBasis {
        self.targeted_with(Some(self.lineage.clone()), targets)
    }

    fn targeted_with(
        &self,
        base: Option<Vec<GrantLineageRef>>,
        targets: Vec<(&str, GrantLineageRef)>,
    ) -> WorkAuthorityBasis {
        WorkAuthorityBasis::TargetedGrantLineages {
            base,
            targets: targets
                .into_iter()
                .map(|(name, grant)| TargetGrantLineage {
                    target: server(name),
                    lineage: vec![grant],
                })
                .collect(),
        }
    }

    fn mcp_policy(&self) -> Arc<GrantBackedWorkPolicy> {
        Arc::new(GrantBackedWorkPolicy::new(
            self.grants.clone(),
            Arc::new(AnyAdmitted),
            Arc::new(McpOperations),
        ))
    }
}

fn allowed(context: &WorkAuthorizationContext, binding: &PreparedAuthorizationBinding) -> bool {
    context.authorization().prepare(binding).is_ok()
}

fn refusal(
    context: &WorkAuthorizationContext,
    binding: &PreparedAuthorizationBinding,
) -> meerkat_core::OperationAuthorizationError {
    context
        .authorization()
        .prepare(binding)
        .map(|_| ())
        .expect_err("operation unexpectedly allowed")
}

fn is_denied(error: &meerkat_core::OperationAuthorizationError) -> bool {
    matches!(
        error,
        meerkat_core::OperationAuthorizationError::Refused(refused)
            if refused.kind() == OperationRefusalKind::Denied
    )
}

#[test]
fn each_target_resolves_only_its_own_selected_lineage() {
    let fixture = Fixture::new();
    let all = fixture.server_grant(
        "alpha-all",
        &[],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let read = fixture.server_grant(
        "beta-read",
        &["read"],
        &["beta"],
        LifetimeRestriction::unrestricted(),
    );
    let association = fixture.original_with(
        "work",
        fixture.targeted(vec![("alpha", all), ("beta", read.clone())]),
    );
    let policy = fixture.mcp_policy();
    let context = fixture.context(&policy, &association);

    // (a) Alpha's lineage is unrestricted in actions; beta's allows only read.
    assert!(allowed(&context, &mcp_call(&[("alpha", "write")])));
    assert!(allowed(&context, &mcp_call(&[("beta", "read")])));
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("beta", "write")])
    )));
    // An operation without a typed target uses the base lineage.
    assert!(allowed(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    ));
    // (c) A server with no selected lineage is denied on its own.
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("gamma", "read")])
    )));
    // (d) An ambiguous plan target is unavailable, never another target's lineage.
    assert_eq!(
        refusal(&context, &mcp_call(&[("alpha", "read"), ("beta", "read")])),
        meerkat_core::OperationAuthorizationError::Unavailable
    );

    // (b) Revoking beta's lineage refuses beta only.
    fixture
        .grants
        .revoke(&who("root"), &read, &mut NoController)
        .expect("revoke beta");
    let context = fixture.context(&policy, &association);
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("beta", "read")])
    )));
    assert!(allowed(&context, &mcp_call(&[("alpha", "write")])));
    assert!(allowed(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    ));
}

#[test]
fn target_lineages_keep_independent_lifetimes() {
    // (g) The clock reads 100: alpha's window ends at 150, beta's never.
    let fixture = Fixture::new();
    let alpha = fixture.server_grant(
        "alpha-window",
        &["read"],
        &["alpha"],
        LifetimeRestriction::window(1, 150),
    );
    let beta = fixture.server_grant(
        "beta-open",
        &["read"],
        &["beta"],
        LifetimeRestriction::unrestricted(),
    );
    let association = fixture.original_with(
        "work",
        fixture.targeted(vec![("alpha", alpha), ("beta", beta)]),
    );
    let policy = fixture.mcp_policy();
    let context = fixture.context(&policy, &association);
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    fixture.clock.0.store(200, Ordering::SeqCst);
    let context = fixture.context(&policy, &association);
    assert!(!allowed(&context, &mcp_call(&[("alpha", "read")])));
    assert!(allowed(&context, &mcp_call(&[("beta", "read")])));
}

#[test]
fn coalesced_originals_each_resolve_their_own_target_lineage() {
    // (e) Both originals select a lineage for alpha; the operation needs both.
    let fixture = Fixture::new();
    let all = fixture.server_grant(
        "alpha-all",
        &[],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let read = fixture.server_grant(
        "alpha-read",
        &["read"],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let first = fixture.original_with("first", fixture.targeted(vec![("alpha", all)]));
    let second = fixture.original_with("second", fixture.targeted(vec![("alpha", read)]));
    let policy = fixture.mcp_policy();
    let context = policy
        .work_context(vec![first, second].into(), scope())
        .expect("context");
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    assert!(!allowed(&context, &mcp_call(&[("alpha", "write")])));
}

#[test]
fn a_single_lineage_still_applies_to_every_target() {
    // (f) A recorded single lineage keeps its meaning: one lineage for every
    // target, so it covers only what that one grant names.
    let fixture = Fixture::new();
    let alpha = fixture.server_grant(
        "alpha-all",
        &[],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let association = fixture.original_with(
        "work",
        WorkAuthorityBasis::GrantLineage {
            lineage: vec![alpha],
        },
    );
    let policy = fixture.mcp_policy();
    let context = fixture.context(&policy, &association);
    assert!(allowed(&context, &mcp_call(&[("alpha", "write")])));
    assert!(!allowed(&context, &mcp_call(&[("beta", "read")])));
}

#[test]
fn the_optional_base_is_neither_a_prerequisite_nor_a_fallback() {
    let fixture = Fixture::new();
    let alpha = fixture.server_grant(
        "alpha-read",
        &["read"],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let policy = fixture.mcp_policy();

    // Without a base, MCP work on a selected target is complete on its own;
    // an operation without a target has no lineage.
    let target_only = fixture.original_with(
        "target-only",
        fixture.targeted_with(None, vec![("alpha", alpha.clone())]),
    );
    let context = fixture.context(&policy, &target_only);
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    assert!(is_denied(&refusal(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    )));

    // Revoking only the base (catalog) lineage leaves the target usable.
    let base = fixture
        .grants
        .issue_root(
            &who("root"),
            id("base-read"),
            who("executor"),
            Some(who("subject")),
            ExecutionRestrictions {
                actions: ExactRestriction::exact([action("read")]),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("base grant");
    let with_base = fixture.original_with(
        "with-base",
        fixture.targeted_with(Some(vec![base.clone()]), vec![("alpha", alpha)]),
    );
    let context = fixture.context(&policy, &with_base);
    assert!(allowed(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    ));
    // A current base covering every read still never stands in for a
    // missing target.
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("beta", "read")])
    )));
    fixture
        .grants
        .revoke(&who("root"), &base, &mut NoController)
        .expect("revoke base");
    let context = fixture.context(&policy, &with_base);
    assert!(is_denied(&refusal(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    )));
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
}

#[test]
fn a_controller_only_selection_refuses_every_operation() {
    let fixture = Fixture::new();
    let association = fixture.original_with("controller-only", fixture.targeted_with(None, vec![]));
    let policy = fixture.mcp_policy();
    let context = fixture.context(&policy, &association);
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("alpha", "read")])
    )));
    assert!(is_denied(&refusal(
        &context,
        &source(SourceAuthorizationUse::Read, "public")
    )));
}

/// A tool catalog control is base work: under a targeted selection, Search
/// and Load resolve the base lineage, and are denied when there is no base
/// or the base excludes the catalog read. A target lineage never covers them.
#[test]
fn catalog_controls_resolve_the_targeted_base() {
    use meerkat_core::tool_execution::CatalogOperation;
    let fixture = Fixture::new();
    let alpha = fixture.server_grant(
        "alpha-any",
        &[],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let policy = fixture.mcp_policy();
    let base_for = |id_name: &str, domain_name: &str| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([action("catalog-read")]),
                    resource_domains: DomainRestriction::exact([domain(domain_name)]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("base grant")
    };

    // A permitted base covers both catalog operations.
    let permitted = fixture.original_with(
        "catalog-permitted",
        fixture.targeted_with(
            Some(vec![base_for("catalog-base", "tool-catalog")]),
            vec![("alpha", alpha.clone())],
        ),
    );
    let context = fixture.context(&policy, &permitted);
    assert!(allowed(&context, &catalog_call(CatalogOperation::Search)));
    assert!(allowed(&context, &catalog_call(CatalogOperation::Load)));

    // Without a base, the target lineage does not stand in for it.
    let absent = fixture.original_with(
        "catalog-absent",
        fixture.targeted_with(None, vec![("alpha", alpha.clone())]),
    );
    let context = fixture.context(&policy, &absent);
    assert!(is_denied(&refusal(
        &context,
        &catalog_call(CatalogOperation::Search)
    )));
    assert!(is_denied(&refusal(
        &context,
        &catalog_call(CatalogOperation::Load)
    )));

    // A base that excludes the catalog's domain denies both.
    let excluding = fixture.original_with(
        "catalog-excluding",
        fixture.targeted_with(
            Some(vec![base_for("other-base", "elsewhere")]),
            vec![("alpha", alpha)],
        ),
    );
    let context = fixture.context(&policy, &excluding);
    assert!(is_denied(&refusal(
        &context,
        &catalog_call(CatalogOperation::Search)
    )));
    assert!(is_denied(&refusal(
        &context,
        &catalog_call(CatalogOperation::Load)
    )));
}

/// A two-link catalog base (the operator's root to an intermediary, then the
/// intermediary's child to the executor) is a whole root-to-leaf chain like
/// any lineage: it covers the catalog controls only while every link covers
/// the catalog read. A child narrowed away from the catalog denies them, so
/// does the child alone without its root, and a revoked root denies them
/// through its unrevoked child.
#[test]
fn a_two_link_catalog_base_covers_the_controls_through_every_link() {
    use meerkat_core::tool_execution::CatalogOperation;
    let fixture = Fixture::new();
    let alpha = fixture.server_grant(
        "alpha-any",
        &[],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let policy = fixture.mcp_policy();
    let catalog_read = |domains: &[&str]| ExecutionRestrictions {
        actions: ExactRestriction::exact([action("catalog-read")]),
        resource_domains: DomainRestriction::exact(domains.iter().map(|name| domain(name))),
        ..ExecutionRestrictions::unrestricted()
    };
    let root = fixture
        .grants
        .issue_root(
            &who("root"),
            id("catalog-root"),
            who("first"),
            Some(who("subject")),
            catalog_read(&["tool-catalog", "elsewhere"]),
        )
        .expect("catalog root");
    let child = fixture
        .grants
        .issue_child(
            &who("first"),
            &root,
            id("catalog-child"),
            who("executor"),
            catalog_read(&["tool-catalog"]),
        )
        .expect("catalog child");
    let narrowed = fixture
        .grants
        .issue_child(
            &who("first"),
            &root,
            id("narrowed-child"),
            who("executor"),
            catalog_read(&["elsewhere"]),
        )
        .expect("narrowed child");
    let with_base = |work: &str, base: Vec<GrantLineageRef>| {
        fixture.original_with(
            work,
            fixture.targeted_with(Some(base), vec![("alpha", alpha.clone())]),
        )
    };
    let denies_both = |association: &InputAuthorityAssociation| {
        let context = fixture.context(&policy, association);
        [CatalogOperation::Search, CatalogOperation::Load]
            .into_iter()
            .all(|operation| is_denied(&refusal(&context, &catalog_call(operation))))
    };

    // Both links cover the catalog read: both controls are allowed.
    let chained = with_base("catalog-chain", vec![root.clone(), child.clone()]);
    let context = fixture.context(&policy, &chained);
    assert!(allowed(&context, &catalog_call(CatalogOperation::Search)));
    assert!(allowed(&context, &catalog_call(CatalogOperation::Load)));

    // The child narrowed the root away from the catalog.
    assert!(denies_both(&with_base(
        "catalog-narrowed",
        vec![root.clone(), narrowed]
    )));

    // The child alone is not the chain the grant owner issued.
    assert!(denies_both(&with_base(
        "catalog-child-only",
        vec![child.clone()]
    )));

    // Revoking the root denies the controls through the unrevoked child.
    fixture
        .grants
        .revoke(&who("root"), &root, &mut NoController)
        .expect("revoke the catalog root");
    assert!(denies_both(&with_base(
        "catalog-revoked-root",
        vec![root, child]
    )));
}

/// A dispatched plan carrying exactly one owner `target`.
fn call_with_target(target: ToolOperationTarget) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(meerkat_core::ToolDef {
            name: "owned_tool".into(),
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
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        )
        .expect("plan")
        .with_owner_witness(
            meerkat_core::ToolExecutionOwnerWitness::new(
                "execution-owner",
                "owned_tool",
                meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
            )
            .expect("owner witness")
            .with_operation_target(target),
        )
        .expect("witness");
    bind(AuthorizationOperation::Tool(ToolAuthorizationFacts {
        call_id: Arc::from("call-1"),
        name: "owned_tool".into(),
        arguments: serde_json::value::RawValue::from_string("{}".to_string())
            .expect("raw arguments")
            .into(),
        target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
    }))
}

/// A registered bundle's operation selects its own exact (bundle,
/// operation) target, never the base lineage; an MCP operation selects its
/// server's lineage.
#[test]
fn a_bundle_operation_selects_its_own_exact_target() {
    assert_eq!(
        operation_grant_target(&call_with_target(
            ToolOperationTarget::RegisteredToolBundleOperation {
                bundle: "mail".into(),
                operation: "send_draft".into(),
                account: std::sync::Arc::new(meerkat_core::OperationAccount::Unavailable),
            }
        )),
        Ok(Some(bundle_operation("mail", "send_draft")))
    );
    assert_eq!(
        operation_grant_target(&call_with_target(ToolOperationTarget::McpServerOperation {
            server: "alpha".into(),
            connection_generation: 1,
            raw_operation: "read".into(),
            connector: meerkat_core::McpConnectorApproval::NoConnector,
            account: std::sync::Arc::new(meerkat_core::OperationAccount::Unavailable),
        })),
        Ok(Some(server("alpha")))
    );
}

fn bundle_operation(bundle: &str, operation: &str) -> GrantTarget {
    GrantTarget::RegisteredToolBundleOperation {
        bundle: bundle.into(),
        operation: operation.into(),
    }
}

/// A dispatched call of the registered bundle `bundle`'s `operation`.
fn bundle_call(bundle: &str, operation: &str) -> PreparedAuthorizationBinding {
    call_with_target(ToolOperationTarget::RegisteredToolBundleOperation {
        bundle: bundle.into(),
        operation: operation.into(),
        account: std::sync::Arc::new(meerkat_core::OperationAccount::Unavailable),
    })
}

/// A targeted selection naming any target kinds, in canonical order.
fn selecting(
    base: Option<Vec<GrantLineageRef>>,
    mut targets: Vec<(GrantTarget, GrantLineageRef)>,
) -> WorkAuthorityBasis {
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    WorkAuthorityBasis::TargetedGrantLineages {
        base,
        targets: targets
            .into_iter()
            .map(|(target, grant)| TargetGrantLineage {
                target,
                lineage: vec![grant],
            })
            .collect(),
    }
}

/// A bundle operation resolves only the lineage selected for exactly that
/// bundle and operation. A catalog-only base, a controller-only selection
/// and an unrelated MCP selection never authorize it, even when their
/// grants would cover its tuple; a sibling operation of the same bundle has
/// no lineage; the exact lineage's own window and revocation refuse only it.
#[test]
fn a_bundle_operation_resolves_only_its_own_exact_lineage() {
    use meerkat_core::tool_execution::CatalogOperation;
    let fixture = Fixture::new();
    let policy = fixture.mcp_policy();
    // Grants that would cover any tuple, if their lineage were selected.
    let broad = |id_name: &str| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions::unrestricted(),
            )
            .expect("broad grant")
    };
    let exact = |id_name: &str, operation: &str, lifetime: LifetimeRestriction| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([action(operation)]),
                    resource_domains: DomainRestriction::exact([domain("bundle-mail")]),
                    lifetime,
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("bundle operation grant")
    };
    let catalog = broad("catalog-base");
    let alpha = broad("alpha-any");

    let catalog_only = fixture.original_with(
        "catalog-only",
        selecting(Some(vec![catalog.clone()]), vec![]),
    );
    let context = fixture.context(&policy, &catalog_only);
    assert!(allowed(&context, &catalog_call(CatalogOperation::Load)));
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("mail", "read_mail")
    )));

    let controller_only = fixture.original_with("controller-only", selecting(None, vec![]));
    let context = fixture.context(&policy, &controller_only);
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("mail", "read_mail")
    )));

    let unrelated = fixture.original_with(
        "unrelated-mcp",
        selecting(None, vec![(server("alpha"), alpha.clone())]),
    );
    let context = fixture.context(&policy, &unrelated);
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("mail", "read_mail")
    )));

    // The exact operation's own lineages, selected beside the others.
    let read = exact(
        "mail-read",
        "read_mail",
        LifetimeRestriction::unrestricted(),
    );
    let list = exact(
        "mail-list",
        "list_mail",
        LifetimeRestriction::window(1, 150),
    );
    let association = fixture.original_with(
        "exact",
        selecting(
            Some(vec![catalog]),
            vec![
                (server("alpha"), alpha),
                (bundle_operation("mail", "read_mail"), read.clone()),
                (bundle_operation("mail", "list_mail"), list),
            ],
        ),
    );
    let context = fixture.context(&policy, &association);
    assert!(allowed(&context, &bundle_call("mail", "read_mail")));
    assert!(allowed(&context, &bundle_call("mail", "list_mail")));
    // A sibling operation and the same operation of another bundle have no
    // lineage of their own; the broad base never stands in.
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("mail", "send_draft")
    )));
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("other", "read_mail")
    )));

    // The list grant's window ends at 150: only the list is refused.
    fixture.clock.0.store(200, Ordering::SeqCst);
    let context = fixture.context(&policy, &association);
    assert!(!allowed(&context, &bundle_call("mail", "list_mail")));
    assert!(allowed(&context, &bundle_call("mail", "read_mail")));

    // Revoking the read grant refuses only the read.
    fixture
        .grants
        .revoke(&who("root"), &read, &mut NoController)
        .expect("revoke read");
    let context = fixture.context(&policy, &association);
    assert!(is_denied(&refusal(
        &context,
        &bundle_call("mail", "read_mail")
    )));
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    assert!(allowed(&context, &catalog_call(CatalogOperation::Search)));
}

fn checked_route(route: &str, server: &str) -> GrantTarget {
    GrantTarget::CheckedMcpRoute {
        route: route.into(),
        server: server.into(),
    }
}

/// A dispatched call of `server`'s raw `operation` reached through the
/// checked route `route`.
fn checked_call(route: &str, server: &str, operation: &str) -> PreparedAuthorizationBinding {
    call_with_target(ToolOperationTarget::CheckedMcpServerOperation {
        route: route.into(),
        server: server.into(),
        connection_generation: 1,
        raw_operation: operation.into(),
        connector: meerkat_core::McpConnectorApproval::NoConnector,
        account: std::sync::Arc::new(meerkat_core::OperationAccount::Unavailable),
    })
}

/// An MCP operation reached through a checked route selects exactly that
/// route to that server, never the server's ordinary target.
#[test]
fn a_checked_route_operation_selects_its_own_exact_target() {
    assert_eq!(
        operation_grant_target(&checked_call("checked", "alpha", "install")),
        Ok(Some(checked_route("checked", "alpha")))
    );
}

/// The identical raw operation is decided by the target it was reached
/// through: the server's ordinary lineage refuses it when it does not name
/// it, while the checked route's own lineage allows it only through that
/// route. The checked lineage never covers the server's ordinary
/// operations, another route or another server, and its own revocation
/// refuses only it.
#[test]
fn a_checked_route_operation_resolves_only_its_own_exact_lineage() {
    let fixture = Fixture::new();
    let policy = fixture.mcp_policy();
    let ordinary = fixture.server_grant(
        "alpha-read",
        &["read"],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let checked = fixture.server_grant(
        "alpha-checked-install",
        &["install"],
        &["alpha"],
        LifetimeRestriction::unrestricted(),
    );
    let association = fixture.original_with(
        "checked",
        selecting(
            None,
            vec![
                (server("alpha"), ordinary),
                (checked_route("checked", "alpha"), checked.clone()),
            ],
        ),
    );
    let context = fixture.context(&policy, &association);
    assert!(allowed(
        &context,
        &checked_call("checked", "alpha", "install")
    ));
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
    // The identical raw operation through the ordinary target is refused
    // by the server's own lineage.
    assert!(is_denied(&refusal(
        &context,
        &mcp_call(&[("alpha", "install")])
    )));
    // The checked lineage never covers an ordinary operation it does not
    // name through the route, another route or another server.
    assert!(is_denied(&refusal(
        &context,
        &checked_call("checked", "alpha", "read")
    )));
    assert!(is_denied(&refusal(
        &context,
        &checked_call("other", "alpha", "install")
    )));
    assert!(is_denied(&refusal(
        &context,
        &checked_call("checked", "beta", "install")
    )));

    // Revoking the checked grant refuses only the checked route.
    fixture
        .grants
        .revoke(&who("root"), &checked, &mut NoController)
        .expect("revoke checked");
    let context = fixture.context(&policy, &association);
    assert!(is_denied(&refusal(
        &context,
        &checked_call("checked", "alpha", "install")
    )));
    assert!(allowed(&context, &mcp_call(&[("alpha", "read")])));
}

fn peer_id(value: u128) -> meerkat_core::comms::PeerId {
    meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(value))
}

/// A dispatched delivery of `kind` to `peer`.
fn peer_call(
    peer: meerkat_core::comms::PeerId,
    kind: meerkat_core::PeerSendKind,
) -> PreparedAuthorizationBinding {
    call_with_target(ToolOperationTarget::PeerSend { peer, kind })
}

fn session_memory(value: u128) -> meerkat_core::MemoryScopeRef {
    meerkat_core::MemoryScopeRef::Session {
        session_id: SessionId::from_uuid(uuid::Uuid::from_u128(value)),
    }
}

/// A dispatched read of the memory `scope`.
fn memory_call(scope: meerkat_core::MemoryScopeRef) -> PreparedAuthorizationBinding {
    call_with_target(ToolOperationTarget::MemoryOperation {
        scope,
        operation: meerkat_core::MemoryOperation::Read,
    })
}

/// A comms delivery selects exactly the peer it reaches, whatever its kind;
/// the roster read selects the peer directory; a memory operation selects
/// exactly its scope. None of them is base work.
#[test]
fn peer_and_memory_operations_select_their_own_exact_targets() {
    use meerkat_core::PeerSendKind;
    for kind in [
        PeerSendKind::Message,
        PeerSendKind::Request,
        PeerSendKind::Response,
    ] {
        assert_eq!(
            operation_grant_target(&peer_call(peer_id(1), kind)),
            Ok(Some(GrantTarget::Peer { peer: peer_id(1) }))
        );
    }
    assert_eq!(
        operation_grant_target(&call_with_target(ToolOperationTarget::PeerDirectoryRead)),
        Ok(Some(GrantTarget::PeerDirectory))
    );
    assert_eq!(
        operation_grant_target(&memory_call(session_memory(7))),
        Ok(Some(GrantTarget::Memory {
            scope: session_memory(7),
        }))
    );
}

/// A delivery resolves only the lineage selected for exactly its peer, the
/// roster read only the peer directory's and a memory read only its
/// scope's. A broad base never covers any of them, another peer or scope
/// has no lineage, the exact grant still bounds the kind of delivery, and
/// revoking one peer's grant refuses only deliveries to that peer.
#[test]
fn peer_and_memory_operations_resolve_only_their_own_exact_lineage() {
    use meerkat_core::PeerSendKind;
    let fixture = Fixture::new();
    let policy = fixture.mcp_policy();
    let broad = |id_name: &str| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions::unrestricted(),
            )
            .expect("broad grant")
    };
    let exact = |id_name: &str, operation: &str, namespace: &str| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(id_name),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([action(operation)]),
                    resource_domains: DomainRestriction::exact([domain(namespace)]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("exact grant")
    };
    let alice = peer_id(1);
    let bob = peer_id(2);

    // A broad base alone covers no delivery, roster read or memory read.
    let base_only = fixture.original_with(
        "base-only",
        selecting(Some(vec![broad("base-only-grant")]), vec![]),
    );
    let context = fixture.context(&policy, &base_only);
    assert!(is_denied(&refusal(
        &context,
        &peer_call(alice, PeerSendKind::Message)
    )));
    assert!(is_denied(&refusal(
        &context,
        &call_with_target(ToolOperationTarget::PeerDirectoryRead)
    )));
    assert!(is_denied(&refusal(
        &context,
        &memory_call(session_memory(7))
    )));

    let to_alice = exact("alice-message", "send-message", &format!("peer-{alice}"));
    let association = fixture.original_with(
        "exact",
        selecting(
            Some(vec![broad("base-grant")]),
            vec![
                (GrantTarget::Peer { peer: alice }, to_alice.clone()),
                (
                    GrantTarget::PeerDirectory,
                    exact("directory-read", "read", "peer-directory"),
                ),
                (
                    GrantTarget::Memory {
                        scope: session_memory(7),
                    },
                    exact(
                        "memory-read",
                        "memory-read",
                        &format!("memory-{}", SessionId::from_uuid(uuid::Uuid::from_u128(7))),
                    ),
                ),
            ],
        ),
    );
    let context = fixture.context(&policy, &association);
    assert!(allowed(&context, &peer_call(alice, PeerSendKind::Message)));
    // The exact grant still bounds the kind of delivery.
    assert!(is_denied(&refusal(
        &context,
        &peer_call(alice, PeerSendKind::Request)
    )));
    // Another peer has no lineage; the broad base never stands in.
    assert!(is_denied(&refusal(
        &context,
        &peer_call(bob, PeerSendKind::Message)
    )));
    assert!(allowed(
        &context,
        &call_with_target(ToolOperationTarget::PeerDirectoryRead)
    ));
    assert!(allowed(&context, &memory_call(session_memory(7))));
    assert!(is_denied(&refusal(
        &context,
        &memory_call(session_memory(8))
    )));

    // Revoking alice's grant refuses only deliveries to alice.
    fixture
        .grants
        .revoke(&who("root"), &to_alice, &mut NoController)
        .expect("revoke alice");
    let context = fixture.context(&policy, &association);
    assert!(is_denied(&refusal(
        &context,
        &peer_call(alice, PeerSendKind::Message)
    )));
    assert!(allowed(
        &context,
        &call_with_target(ToolOperationTarget::PeerDirectoryRead)
    ));
    assert!(allowed(&context, &memory_call(session_memory(7))));
}
