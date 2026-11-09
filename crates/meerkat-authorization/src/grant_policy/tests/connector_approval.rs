//! The connector approval at the operation boundary, through the real grant
//! owner and the real resolved-grant operation check: the work's own grant
//! must explicitly list the approval of the connection the operation runs
//! on. A wildcard action bound (every raw operation) approves none, a
//! recorded other approval is a denial, and an unrecorded one is missing
//! evidence. The operation owner here is a test fixture producing the stock
//! tuple shape (raw operation plus connector approval).
use super::*;
use crate::mcp_installation::{mcp_connector_action, mcp_connector_domain};
use meerkat_core::McpConnectorApproval;

/// Produces, for any source read, the raw operation and the connector
/// approval of the connection it runs on, as the stock owner does for an
/// MCP operation.
struct ConnectorOwner {
    /// The owner of the installation the connection belongs to.
    authority: PrincipalRef,
    approval: McpConnectorApproval,
}
impl OperationPolicyOwner for ConnectorOwner {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let connector = mcp_connector_action("sentinel", &self.approval);
        let tuple = |action: ActionRef| LocalOperationValues {
            action,
            resource_domain: ResourceDomain {
                authority: self.authority.clone(),
                namespace: "mcp-server/sentinel".into(),
            },
            processor: ProcessorRef::Principal {
                principal: who("executor"),
            },
            audience: AudienceRef::Principal {
                principal: who("requester"),
            },
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![
                tuple(ActionRef {
                    feature: "mcp".into(),
                    action: "read".into(),
                }),
                tuple(connector),
            ],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

fn approval(account: &str) -> McpConnectorApproval {
    McpConnectorApproval::Connector(format!("sha256:{account}"))
}

/// The verdict on an operation over a connection with `actual`, for work
/// whose grant has `restrictions`.
fn verdict(
    fixture: &Fixture,
    label: &str,
    restrictions: ExecutionRestrictions,
    actual: &McpConnectorApproval,
) -> Result<(), meerkat_core::OperationAuthorizationError> {
    let grant = fixture
        .grants
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &who("root"),
            id(label),
            who("executor"),
            Some(who("subject")),
            restrictions,
        )
        .expect("grant");
    operate(
        fixture,
        &fixture.grants,
        vec![grant],
        &who("resource"),
        actual,
    )
}

/// The real work policy's verdict on an operation over a connection with
/// `actual` to `authority`'s `sentinel`, for work whose grant is the
/// retained `lineage` of `grants`.
fn operate(
    fixture: &Fixture,
    grants: &Arc<LocalGrantAuthority>,
    lineage: Vec<GrantLineageRef>,
    authority: &PrincipalRef,
    actual: &McpConnectorApproval,
) -> Result<(), meerkat_core::OperationAuthorizationError> {
    let mut candidate = fixture.association().candidate().clone();
    candidate.authority_basis = WorkAuthorityBasis::GrantLineage { lineage };
    let association = InputAuthorityAssociation::new(candidate).expect("association");
    let admitted = Arc::new(Admitted {
        original: association.clone(),
        requester_allowed: AtomicBool::new(true),
        ordinary_allowed: AtomicBool::new(true),
        controller_allowed: AtomicBool::new(true),
        calls: Mutex::new(vec![]),
    });
    let policy = GrantBackedWorkPolicy::new(
        grants.clone(),
        admitted,
        Arc::new(ConnectorOwner {
            authority: authority.clone(),
            approval: actual.clone(),
        }),
    );
    policy
        .evaluate(
            &association,
            &source(SourceAuthorizationUse::Read, "mcp-server/sentinel"),
            150,
        )
        .map(drop)
}

fn every_operation(domains: Vec<ResourceDomain>) -> ExecutionRestrictions {
    ExecutionRestrictions {
        resource_domains: DomainRestriction::exact(domains),
        lifetime: LifetimeRestriction::window(100, 300),
        ..ExecutionRestrictions::unrestricted()
    }
}

fn exact_operations(extra: Option<ActionRef>) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact(
            std::iter::once(ActionRef {
                feature: "mcp".into(),
                action: "read".into(),
            })
            .chain(extra),
        ),
        resource_domains: DomainRestriction::exact([domain("mcp-server/sentinel")]),
        lifetime: LifetimeRestriction::window(100, 300),
        ..ExecutionRestrictions::unrestricted()
    }
}

fn is_denied(result: &Result<(), meerkat_core::OperationAuthorizationError>) -> bool {
    matches!(
        result,
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::Denied
    )
}

#[test]
fn a_wildcard_operation_grant_approves_only_the_connector_its_domains_name() {
    let fixture = Fixture::new();
    let account_a = approval("aa");
    let account_b = approval("bb");
    let none = McpConnectorApproval::NoConnector;

    // Every raw operation, no approval recorded: missing evidence for any
    // connection, never an approval through the wildcard.
    for (label, actual) in [
        ("all-a", &account_a),
        ("all-b", &account_b),
        ("all-n", &none),
    ] {
        assert_eq!(
            verdict(
                &fixture,
                label,
                every_operation(vec![domain("mcp-server/sentinel")]),
                actual
            ),
            Err(meerkat_core::OperationAuthorizationError::Unavailable),
            "{label}"
        );
    }

    // Every raw operation with account A approved in its domains.
    let approved_a = || {
        every_operation(vec![
            domain("mcp-server/sentinel"),
            mcp_connector_domain(&who("resource"), "sentinel", &account_a),
        ])
    };
    assert_eq!(
        verdict(&fixture, "all-approved-a", approved_a(), &account_a),
        Ok(())
    );
    for (label, actual) in [("all-approved-b", &account_b), ("all-approved-n", &none)] {
        assert!(
            is_denied(&verdict(&fixture, label, approved_a(), actual)),
            "{label}: a same-name connection bound to another account is refused"
        );
    }
}

/// An owner-scoped domain bound records no connector approval. It covers
/// every namespace of the installation's owner, but it names no approval in
/// the domain form and identifies no one installation for the action form,
/// so a connection under it is missing evidence, never an approval adopted
/// from the connection.
#[test]
fn an_owner_scoped_domain_bound_records_no_connector_approval() {
    let fixture = Fixture::new();
    let account_a = approval("aa");
    let owner_scoped = |mut restrictions: ExecutionRestrictions| {
        restrictions.resource_domains = DomainRestriction::owned_by([who("resource")]);
        restrictions
    };
    for (label, restrictions, actual) in [
        (
            "owned-all-a",
            owner_scoped(every_operation(vec![])),
            &account_a,
        ),
        (
            "owned-all-n",
            owner_scoped(every_operation(vec![])),
            &McpConnectorApproval::NoConnector,
        ),
        (
            "owned-action-a",
            owner_scoped(exact_operations(Some(mcp_connector_action(
                "sentinel", &account_a,
            )))),
            &account_a,
        ),
    ] {
        assert_eq!(
            verdict(&fixture, label, restrictions, actual),
            Err(meerkat_core::OperationAuthorizationError::Unavailable),
            "{label}"
        );
    }
}

#[test]
fn an_exact_operation_grant_approves_only_its_listed_connector() {
    let fixture = Fixture::new();
    let account_a = approval("aa");
    let account_b = approval("bb");
    let with_a = || exact_operations(Some(mcp_connector_action("sentinel", &account_a)));
    assert_eq!(verdict(&fixture, "exact-a", with_a(), &account_a), Ok(()));
    assert!(is_denied(&verdict(
        &fixture,
        "exact-b",
        with_a(),
        &account_b
    )));
    assert!(is_denied(&verdict(
        &fixture,
        "exact-n",
        with_a(),
        &McpConnectorApproval::NoConnector
    )));
    // A grant issued before approvals were recorded: the same missing
    // evidence as at attachment, not a denial.
    assert_eq!(
        verdict(&fixture, "legacy", exact_operations(None), &account_a),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    );
}

fn installation_of(owner: &str) -> ResourceDomain {
    ResourceDomain {
        authority: who(owner),
        namespace: "mcp-server/sentinel".into(),
    }
}

/// A root (to `first`) and its child (to the executor) retained by a durable
/// grant owner and restored from its store. `stored_child` stands in for the
/// child's bounds as an earlier owner version accepted them (narrower than
/// the root, so restore still validates them); `None` keeps the bounds the
/// current owner issued.
fn stored_chain(
    fixture: &Fixture,
    label: &str,
    root: ExecutionRestrictions,
    middle: Option<ExecutionRestrictions>,
    stored_child: Option<ExecutionRestrictions>,
) -> (Arc<LocalGrantAuthority>, Vec<GrantLineageRef>) {
    let store = Arc::new(crate::grants::InMemoryGrantOwnerStore::new());
    let owner = LocalGrantAuthority::new_durable(
        &meerkat_core::AdmissionGuard::none(),
        LocalGrantConfiguration {
            root: who("root"),
            namespace: id(label),
            generation: 1,
        },
        store.clone(),
        LocalAuthorizationPublication::new(),
        fixture.clock.clone(),
    )
    .expect("durable owner");
    let root = owner
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &who("root"),
            id("root"),
            who("first"),
            Some(who("subject")),
            root,
        )
        .expect("root");
    // An optional intermediate grant (to `second`) between root and leaf.
    let mut lineage = vec![root.clone()];
    let (parent, issuer) = match middle {
        Some(requested) => {
            let middle = owner
                .issue_child(
                    &meerkat_core::AdmissionGuard::none(),
                    &who("first"),
                    &root,
                    id("middle"),
                    who("second"),
                    requested,
                )
                .expect("middle");
            lineage.push(middle.clone());
            (middle, "second")
        }
        None => (root, "first"),
    };
    let child = owner
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            &who(issuer),
            &parent,
            id("child"),
            who("executor"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("child");
    lineage.push(child);
    drop(owner);
    let store: Arc<crate::grants::InMemoryGrantOwnerStore> = match stored_child {
        Some(restrictions) => crate::grants::with_last_issued_restrictions(&store, restrictions)
            .expect("stored child"),
        None => store,
    };
    let restored = LocalGrantAuthority::restore(
        store,
        LocalAuthorizationPublication::new(),
        fixture.clock.clone(),
    )
    .expect("restore the retained chain");
    (Arc::new(restored), lineage)
}

/// The attachment-boundary check on the retained chain for `owner`'s
/// `sentinel` with `approval`.
fn attach(
    grants: &LocalGrantAuthority,
    lineage: &[GrantLineageRef],
    owner: &str,
    approval: &McpConnectorApproval,
) -> Result<(), meerkat_authorization_contracts::constraints::RestrictionFailure> {
    let resolved = grants
        .resolve_lineage(lineage, &who("executor"), Some(&who("subject")))
        .expect("resolve the retained chain");
    crate::mcp_installation::check_resolved_installation(
        &resolved,
        &crate::mcp_installation::McpInstallationRef::new(
            installation_of(owner),
            "sentinel",
            approval,
        )
        .expect("installation"),
        150,
    )
}

/// The leaf's own bounds alone, without its retained ancestors.
fn leaf_alone(
    grants: &LocalGrantAuthority,
    lineage: &[GrantLineageRef],
    owner: &str,
    approval: &McpConnectorApproval,
) -> crate::mcp_installation::McpConnectorBound {
    let resolved = grants
        .resolve_lineage(lineage, &who("executor"), Some(&who("subject")))
        .expect("resolve the retained chain");
    crate::mcp_installation::connector_bound(
        resolved.restrictions(),
        &mcp_connector_action("sentinel", approval),
        &installation_of(owner),
    )
}

/// One durable grant owner, both boundaries: approvals keyed by installation
/// authority (owner A's aa and owner B's bb on one root, cross pairs
/// refused), and retained chains an earlier owner version accepted are
/// checked against every link, not the leaf alone (a child replacing the
/// root's approval is a mismatch; a child promoting an approval its root
/// never recorded is missing evidence).
#[test]
fn retained_chains_are_checked_against_every_link_at_both_boundaries() {
    let fixture = Fixture::new();
    let (aa, bb) = (approval("aa"), approval("bb"));
    let bounded = |domains: Vec<ResourceDomain>| ExecutionRestrictions {
        resource_domains: DomainRestriction::exact(domains),
        lifetime: LifetimeRestriction::window(100, 300),
        ..ExecutionRestrictions::unrestricted()
    };
    let denied_verdict =
        |result: Result<(), meerkat_core::OperationAuthorizationError>| is_denied(&result);
    // The raw read and the installation are always covered, so a refusal is
    // the approval's alone.
    let covered = |approval: &McpConnectorApproval| {
        ExactRestriction::exact([
            ActionRef {
                feature: "mcp".into(),
                action: "read".into(),
            },
            ActionRef {
                feature: crate::mcp_installation::MCP_INSTALLATION_FEATURE.into(),
                action: "sentinel".into(),
            },
            mcp_connector_action("sentinel", approval),
        ])
    };

    // Two owners' installations of the same server name on one root.
    let (grants, lineage) = stored_chain(
        &fixture,
        "two-owners",
        bounded(vec![
            installation_of("owner-a"),
            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
            installation_of("owner-b"),
            mcp_connector_domain(&who("owner-b"), "sentinel", &bb),
        ]),
        None,
        None,
    );
    for (owner, approval) in [("owner-a", &aa), ("owner-b", &bb)] {
        assert_eq!(
            attach(&grants, &lineage, owner, approval),
            Ok(()),
            "{owner}"
        );
        assert_eq!(
            operate(&fixture, &grants, lineage.clone(), &who(owner), approval),
            Ok(()),
            "{owner}"
        );
    }
    for (owner, approval) in [("owner-a", &bb), ("owner-b", &aa)] {
        assert!(
            attach(&grants, &lineage, owner, approval).is_err(),
            "{owner}"
        );
        assert!(
            denied_verdict(operate(
                &fixture,
                &grants,
                lineage.clone(),
                &who(owner),
                approval
            )),
            "{owner}: another owner's approval"
        );
    }

    // A stored child that dropped the root's record of aa and names bb by
    // action: the leaf alone would approve bb, the retained root refuses it.
    let replaced = ExecutionRestrictions {
        actions: covered(&bb),
        ..bounded(vec![installation_of("owner-a")])
    };
    let (grants, lineage) = stored_chain(
        &fixture,
        "replaced",
        bounded(vec![
            installation_of("owner-a"),
            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
        ]),
        None,
        Some(replaced),
    );
    assert_eq!(
        leaf_alone(&grants, &lineage, "owner-a", &bb),
        crate::mcp_installation::McpConnectorBound::Approved,
        "the leaf alone would approve bb"
    );
    assert!(attach(&grants, &lineage, "owner-a", &bb).is_err());
    assert!(denied_verdict(operate(
        &fixture,
        &grants,
        lineage,
        &who("owner-a"),
        &bb
    )));

    // A stored child naming aa under a root that recorded no approval: the
    // leaf alone would approve it; the missing root approval stays missing.
    let promoted = ExecutionRestrictions {
        actions: covered(&aa),
        ..bounded(vec![installation_of("owner-a")])
    };
    let (grants, lineage) = stored_chain(
        &fixture,
        "promoted",
        bounded(vec![installation_of("owner-a")]),
        None,
        Some(promoted.clone()),
    );
    assert_eq!(
        leaf_alone(&grants, &lineage, "owner-a", &aa),
        crate::mcp_installation::McpConnectorBound::Approved,
        "the leaf alone would approve aa"
    );
    assert!(attach(&grants, &lineage, "owner-a", &aa).is_err());
    assert_eq!(
        operate(&fixture, &grants, lineage, &who("owner-a"), &aa),
        Err(meerkat_core::OperationAuthorizationError::Unavailable),
        "missing evidence, not a mismatch"
    );

    // Root approves aa, an intermediate grant removed it, and a stored leaf
    // names aa again: the removal ends it at both boundaries.
    let (grants, lineage) = stored_chain(
        &fixture,
        "middle-removed",
        bounded(vec![
            installation_of("owner-a"),
            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
        ]),
        Some(bounded(vec![installation_of("owner-a")])),
        Some(promoted),
    );
    assert_eq!(lineage.len(), 3);
    assert_eq!(
        leaf_alone(&grants, &lineage, "owner-a", &aa),
        crate::mcp_installation::McpConnectorBound::Approved,
        "the leaf alone would approve aa"
    );
    assert!(attach(&grants, &lineage, "owner-a", &aa).is_err());
    assert_eq!(
        operate(&fixture, &grants, lineage, &who("owner-a"), &aa),
        Err(meerkat_core::OperationAuthorizationError::Unavailable),
        "the removal is missing evidence for every descendant"
    );
    // The same account through a keeping intermediate stays delegable.
    let (grants, lineage) = stored_chain(
        &fixture,
        "middle-kept",
        bounded(vec![
            installation_of("owner-a"),
            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
        ]),
        Some(ExecutionRestrictions::unrestricted()),
        None,
    );
    assert_eq!(attach(&grants, &lineage, "owner-a", &aa), Ok(()));
    assert_eq!(
        operate(&fixture, &grants, lineage, &who("owner-a"), &aa),
        Ok(())
    );

    // A two-owner root narrowed by its grantee to owner A, naming A's
    // approval in both forms: issued through the real issue_child and
    // admitted for A at both boundaries; owner B was narrowed away.
    let (grants, lineage) = stored_chain(
        &fixture,
        "narrowed-to-a",
        bounded(vec![
            installation_of("owner-a"),
            mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
            installation_of("owner-b"),
            mcp_connector_domain(&who("owner-b"), "sentinel", &bb),
        ]),
        Some(ExecutionRestrictions {
            actions: covered(&aa),
            ..bounded(vec![
                installation_of("owner-a"),
                mcp_connector_domain(&who("owner-a"), "sentinel", &aa),
            ])
        }),
        None,
    );
    assert_eq!(attach(&grants, &lineage, "owner-a", &aa), Ok(()));
    assert_eq!(
        operate(&fixture, &grants, lineage.clone(), &who("owner-a"), &aa),
        Ok(())
    );
    assert!(attach(&grants, &lineage, "owner-b", &bb).is_err());
    assert!(operate(&fixture, &grants, lineage, &who("owner-b"), &bb).is_err());

    // An action-form approval on a grant holding two owners' installations of
    // the same server name identifies neither: it approves no connector.
    let (grants, lineage) = stored_chain(
        &fixture,
        "ambiguous-action",
        ExecutionRestrictions {
            actions: covered(&aa),
            ..bounded(vec![installation_of("owner-a"), installation_of("owner-b")])
        },
        None,
        None,
    );
    for owner in ["owner-a", "owner-b"] {
        assert!(attach(&grants, &lineage, owner, &aa).is_err(), "{owner}");
        assert_eq!(
            operate(&fixture, &grants, lineage.clone(), &who(owner), &aa),
            Err(meerkat_core::OperationAuthorizationError::Unavailable),
            "{owner}: an ambiguous action-form approval is not one"
        );
    }
}
