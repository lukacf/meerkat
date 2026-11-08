//! Installed policy component controls with real enrollment, native grants,
//! SQLite storage and host lifetime. Typed operation facts are test inputs;
//! these controls do not attach MCP, dispatch ServiceCall or contact a provider.
use super::*;
use crate::original_ingress::tests::{initialized, principal};
use crate::private_originals::collector_setup::{check_grant, tests as enrollment};
use meerkat_authorization_contracts::constraints::{ActionRef, ProcessorRef};
use meerkat_core::{
    OperationAccount, QualifiedAccount, ToolOperationTarget,
    authorization::{OperationAuthorizationFacts, ToolAuthorizationFacts, ToolAuthorizationTarget},
    exact_operation::OperationExecutionScope,
    mcp_config::McpConnectorApproval,
};
use meerkat_runtime::meerkat_machine::AuthenticatedOriginalSource;
use meerkat_toolkit::host_lifetime::{
    control::{HostCloseReason, PrivateHostLifetimeControl},
    policy::CompiledLocalPolicy,
};
use std::time::Duration;
use toolkit_store::{StoreKey, StoreScope, ToolkitStore};

fn binding(target: ToolOperationTarget) -> PreparedAuthorizationBinding {
    // As in the existing MCP policy component tests, the typed target is
    // supplied directly. It is not a native router/connection witness.
    let fingerprint = meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
        &meerkat_core::ToolCatalogEntry::session_inline(
            Arc::new(meerkat_core::ToolDef::new(
                "policy-input",
                "collector policy input",
                serde_json::json!({"type": "object"}),
            )),
            true,
        ),
    );
    let witness = meerkat_core::ToolExecutionOwnerWitness::new(
        "collector-policy-fixture",
        "selected-read",
        fingerprint,
    )
    .unwrap()
    .with_operation_target(target);
    let deadlines =
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(60),
        )])
        .unwrap();
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(deadlines)
        .unwrap()
        .with_owner_witness(witness)
        .unwrap();
    PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000007\"").unwrap(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
            call_id: Arc::from("collector-policy-call"),
            // A display name is deliberately not used as the permission key.
            name: "policy-input".into(),
            arguments: serde_json::value::RawValue::from_string("{}".into())
                .unwrap()
                .into(),
            target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
        }),
    })
}

fn denied<T>(result: Result<T, OperationAuthorizationError>) {
    assert!(
        matches!(result, Err(OperationAuthorizationError::Refused(refusal))
        if refusal.kind() == OperationRefusalKind::Denied)
    );
}

#[tokio::test]
async fn enrolled_collector_policy_requires_exact_read_target_and_open_host() {
    collector_case(false).await;
}

#[tokio::test]
async fn enrolled_collector_review_floor_preserves_exact_read_target_and_refusals() {
    collector_case(true).await;
}

struct CollectorFloor {
    calls: std::sync::atomic::AtomicUsize,
    expected: ToolOperationTarget,
}
impl InstalledOperationReviewPolicy for CollectorFloor {
    fn review_floor(
        &self,
        context: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError> {
        assert_eq!(context.target(), &self.expected);
        assert_eq!(
            context.selected().native_host.as_ref(),
            Some(context.native())
        );
        assert_eq!(
            context.association().candidate().ingress_namespace.realm,
            context.native().realm
        );
        assert_eq!(context.allowance().review_tier, OperationReviewTier::R1);
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(OperationReviewTier::R3)
    }
}
async fn collector_case(select_floor: bool) {
    let (directory, mut selection) = super::super::tests::fixture();
    let grant_path = directory.path().join("collector-grants.jsonl");
    let source = initialized(&grant_path);
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    // Select the existing collector fixture's actual ingress owner before
    // host bootstrap. Its agent route is unrelated to this service original.
    local.native_host.as_mut().unwrap().ingress_domain = source.ingress_domain().clone();
    super::super::tests::replace_local(&mut selection, &local);
    let native = local.native_host.as_ref().unwrap().clone();
    let mut lifetime = NativeHostLifetime::bootstrap(selection).unwrap();
    lifetime
        .bind_native_authorization_publication(source.publication())
        .unwrap();
    let lifetime = Arc::new(lifetime);
    let scope = StoreScope {
        realm: native.realm.to_string(),
        owner: local.deployment.to_string(),
    };
    let store: Arc<dyn ToolkitStore> =
        Arc::new(SqliteStore::open(directory.path().join("collector-source.sqlite3")).unwrap());
    let originals = Arc::new(
        PrivateOriginals::prepare(scope.clone(), store.clone(), source.clone())
            .await
            .unwrap(),
    );
    source
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    originals.bind_host_lifetime(&lifetime).unwrap();
    let mut record = enrollment::record(&source, &principal("collector"));
    record.association.ingress_namespace.realm = native.realm.clone();
    originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    let association = source
        .enrolled_service_original(
            &record.association.ingress_namespace,
            &record.association.original_work,
        )
        .unwrap()
        .unwrap()
        .association()
        .clone();
    let key = StoreKey {
        realm: scope.realm,
        owner: scope.owner,
        id: record.id.as_str().into(),
    };
    let stored = store.get(&key).await.unwrap().unwrap();
    let grants = fs::read(&grant_path).unwrap();
    assert!(
        native
            .members
            .iter()
            .all(|member| member.logical_owner != record.association.logical_executor)
    );
    let (config, _, _) = enrollment::fixture();
    let target = ToolOperationTarget::McpServerOperation {
        server: "gmail".into(),
        connection_generation: 1,
        raw_operation: "history_page".into(),
        connector: config.connector_approval(),
        account: Arc::new(OperationAccount::Selected(
            QualifiedAccount::new("https://accounts.google.com", "verified-subject").unwrap(),
        )),
    };
    let floor = Arc::new(CollectorFloor {
        calls: std::sync::atomic::AtomicUsize::new(0),
        expected: target.clone(),
    });
    let policy = InstalledApplicationPolicy {
        operation_review_policy: select_floor
            .then(|| floor.clone() as Arc<dyn InstalledOperationReviewPolicy>),
        originals,
        lifetime: lifetime.clone(),
        native,
        selected: local,
    };
    let read = binding(target.clone());
    let ((work, operation), _) = source
        .publication()
        .observe(|| {
            (
                policy.authorize_admitted_work(
                    &association,
                    &read,
                    LocalPolicyPurpose::Operation,
                    7,
                ),
                policy.authorize_operation(&association, &read, LocalPolicyPurpose::Operation, 7),
            )
        })
        .unwrap();
    let work = work.unwrap();
    assert_eq!(work.expires_at_ms, 1_800_007);
    assert_eq!(work.restrictions, ExecutionRestrictions::unrestricted());
    let operation = operation.unwrap();
    assert_eq!(operation.expires_at_ms, 1_800_007);
    assert_eq!(
        operation.restrictions,
        ExecutionRestrictions::unrestricted()
    );
    assert_eq!(
        operation.review_tier,
        if select_floor {
            OperationReviewTier::R3
        } else {
            OperationReviewTier::R1
        }
    );
    assert_eq!(operation.operation_values.len(), 2);
    let read_values = &operation.operation_values[0];
    assert_eq!(
        read_values.action,
        ActionRef {
            feature: "mcp".into(),
            action: "history_page".into()
        }
    );
    assert_eq!(
        read_values.resource_domain,
        ResourceDomain {
            authority: source.ingress_domain().authority.clone(),
            namespace: "mcp-server/gmail".into(),
        }
    );
    let connector_values = &operation.operation_values[1];
    assert_eq!(
        connector_values.action,
        meerkat_authorization::mcp_installation::mcp_connector_action(
            "gmail",
            &config.connector_approval(),
        )
    );
    // Invocation names the installation; the native lineage separately records
    // the approved connector in its exact resource domains.
    assert_eq!(
        connector_values.resource_domain,
        read_values.resource_domain
    );
    let approval_domain = meerkat_authorization::mcp_installation::mcp_connector_domain(
        &source.ingress_domain().authority,
        "gmail",
        &config.connector_approval(),
    );
    let grant = source
        .grants()
        .resolve_lineage(&record.lineage, &record.association.logical_executor, None)
        .unwrap();
    let meerkat_authorization_contracts::constraints::DomainBound::Exact(domains) =
        grant.restrictions().resource_domains.bound()
    else {
        panic!("exact collector domains required")
    };
    assert!(domains.contains(&approval_domain));
    assert_eq!(
        meerkat_authorization::mcp_installation::lineage_connector_bound(
            grant.lineage_restrictions(),
            grant.restrictions(),
            &connector_values.action,
            &connector_values.resource_domain,
        ),
        meerkat_authorization::mcp_installation::McpConnectorBound::Approved
    );
    let now = source.clock().now().unwrap().unix_ms;
    let before_floor = floor.calls.load(std::sync::atomic::Ordering::SeqCst);
    let ((mandate, account), stamp) = source
        .publication()
        .observe(|| {
            (
                policy.review_mandate(&association, &read, now),
                policy.review_action_account(&association, &read, now),
            )
        })
        .unwrap();
    let mandate = mandate.unwrap().unwrap();
    assert!(mandate.contains("installed collector policy"));
    let actual_bounds = meerkat_runtime::meerkat_machine::review_bounds_text(
        &grant
            .restrictions()
            .conjoin(&association.candidate().admitted_ceiling),
        grant.expires_at_ms(),
        now,
    )
    .unwrap();
    assert!(mandate.contains(&actual_bounds));
    assert_eq!(
        account.unwrap().as_deref(),
        Some("verified-subject at https://accounts.google.com")
    );
    stamp.check_current().unwrap();
    assert_eq!(
        floor.calls.load(std::sync::atomic::Ordering::SeqCst),
        before_floor
    );
    for values in &operation.operation_values {
        assert_eq!(
            values.processor,
            ProcessorRef::Principal {
                principal: record.association.logical_executor.clone()
            }
        );
        assert_eq!(values.audience, record.association.target.audience);
        assert!(
            grant
                .restrictions()
                .check_bounds(
                    meerkat_authorization_contracts::constraints::OperationRestrictionValues {
                        action: &values.action,
                        resource_domain: &values.resource_domain,
                        processor: &values.processor,
                        audience: &values.audience,
                        now_ms: now,
                    },
                )
                .is_ok()
        );
    }

    for change in [
        "server",
        "operation",
        "approval",
        "issuer",
        "subject",
        "no_account",
    ] {
        let mut foreign = target.clone();
        let ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            connector,
            account,
            ..
        } = &mut foreign
        else {
            unreachable!()
        };
        match change {
            "server" => *server = "other-gmail".into(),
            "operation" => *raw_operation = "send_email".into(),
            "approval" => *connector = McpConnectorApproval::Connector("other-installation".into()),
            "issuer" => {
                *account = Arc::new(OperationAccount::Selected(
                    QualifiedAccount::new("https://other.invalid", "verified-subject").unwrap(),
                ))
            }
            "subject" => {
                *account = Arc::new(OperationAccount::Selected(
                    QualifiedAccount::new("https://accounts.google.com", "other-subject").unwrap(),
                ))
            }
            "no_account" => *account = Arc::new(OperationAccount::Unavailable),
            _ => unreachable!(),
        }
        let (result, _) = source
            .publication()
            .observe(|| {
                policy.authorize_operation(
                    &association,
                    &binding(foreign),
                    LocalPolicyPurpose::Operation,
                    7,
                )
            })
            .unwrap();
        denied(result);
    }
    let ((work, operation), _) = source
        .publication()
        .observe(|| {
            (
                policy.authorize_admitted_work(
                    &association,
                    &read,
                    LocalPolicyPurpose::Controller,
                    7,
                ),
                policy.authorize_operation(&association, &read, LocalPolicyPurpose::Controller, 7),
            )
        })
        .unwrap();
    denied(work);
    denied(operation);

    lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    let ((work, operation), _) = source
        .publication()
        .observe(|| {
            (
                policy.authorize_admitted_work(
                    &association,
                    &read,
                    LocalPolicyPurpose::Operation,
                    7,
                ),
                policy.authorize_operation(&association, &read, LocalPolicyPurpose::Operation, 7),
            )
        })
        .unwrap();
    denied(work);
    denied(operation);
    // The close fence refuses the same otherwise valid enrolled source. It
    // does not fabricate cancellation, revoke a grant or mutate enrollment.
    let ((mandate, account), _) = source
        .publication()
        .observe(|| {
            (
                policy.review_mandate(&association, &read, now),
                policy.review_action_account(&association, &read, now),
            )
        })
        .unwrap();
    denied(mandate);
    denied(account);
    assert!(store.get(&key).await.unwrap().unwrap() == stored);
    assert_eq!(
        floor.calls.load(std::sync::atomic::Ordering::SeqCst),
        usize::from(select_floor),
        "only the exact authorized collector read reaches the floor"
    );
    assert_eq!(fs::read(&grant_path).unwrap(), grants);
    assert!(check_grant(&source, &record).is_ok());
}

// A real published collector and installed lifetime, using the same typed
// policy-only binding as the existing controls above. No native dispatch.
async fn collector_work_read_fixture() -> (
    tempfile::TempDir,
    Arc<ToolkitOriginalOwner>,
    InstalledApplicationPolicy,
    InputAuthorityAssociation,
    PreparedAuthorizationBinding,
) {
    let (directory, mut selection) = super::super::tests::fixture();
    let source = initialized(&directory.path().join("work-read-grants"));
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    local.native_host.as_mut().unwrap().ingress_domain = source.ingress_domain().clone();
    super::super::tests::replace_local(&mut selection, &local);
    let native = local.native_host.as_ref().unwrap().clone();
    let mut lifetime = NativeHostLifetime::bootstrap(selection).unwrap();
    lifetime
        .bind_native_authorization_publication(source.publication())
        .unwrap();
    let lifetime = Arc::new(lifetime);
    let scope = StoreScope {
        realm: native.realm.to_string(),
        owner: local.deployment.to_string(),
    };
    let store: Arc<dyn ToolkitStore> =
        Arc::new(SqliteStore::open(directory.path().join("work-read-source.sqlite3")).unwrap());
    let originals = Arc::new(
        PrivateOriginals::prepare(scope.clone(), store.clone(), source.clone())
            .await
            .unwrap(),
    );
    source
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    originals.bind_host_lifetime(&lifetime).unwrap();
    let mut record = enrollment::record(&source, &principal("collector"));
    record.association.ingress_namespace.realm = native.realm.clone();
    originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    let association = source
        .enrolled_service_original(
            &record.association.ingress_namespace,
            &record.association.original_work,
        )
        .unwrap()
        .unwrap()
        .association()
        .clone();
    let (config, _, _) = enrollment::fixture();
    let read = binding(ToolOperationTarget::McpServerOperation {
        server: "gmail".into(),
        connection_generation: 1,
        raw_operation: "history_page".into(),
        connector: config.connector_approval(),
        account: Arc::new(OperationAccount::Selected(
            QualifiedAccount::new("https://accounts.google.com", "verified-subject").unwrap(),
        )),
    });
    let policy = InstalledApplicationPolicy {
        operation_review_policy: None,
        originals,
        lifetime,
        native,
        selected: local,
    };
    (directory, source, policy, association, read)
}

#[tokio::test]
async fn admitted_work_collector_read_waits_and_keeps_actual_allowance() {
    use std::sync::mpsc::{self, RecvTimeoutError};
    const BOUND: Duration = Duration::from_secs(2);
    let (_directory, source, policy, association, read) = collector_work_read_fixture().await;
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let (locked, entered, waited, result, released, holder, reader) = std::thread::scope(|scope| {
        let source_ref = &source;
        let holder = scope.spawn(move || {
            let guard = source_ref.lock_originals_for_test();
            let announced = locked_tx.send(()).is_ok();
            let released = release_rx.recv_timeout(BOUND).is_ok();
            drop(guard);
            (announced, released)
        });
        let locked = locked_rx.recv_timeout(BOUND);
        let reader = scope.spawn(|| {
            let announced = entered_tx.send(()).is_ok();
            let result = source.publication().observe(|| {
                policy.authorize_admitted_work(
                    &association,
                    &read,
                    LocalPolicyPurpose::Operation,
                    7,
                )
            });
            let delivered = result_tx.send(result).is_ok();
            (announced, delivered)
        });
        let entered = entered_rx.recv_timeout(BOUND);
        let early = result_rx.recv_timeout(Duration::from_millis(50));
        let waited = matches!(&early, Err(RecvTimeoutError::Timeout));
        let released = release_tx.send(());
        let result = match early {
            Err(RecvTimeoutError::Timeout) => result_rx.recv_timeout(BOUND),
            value => value,
        };
        (
            locked,
            entered,
            waited,
            result,
            released,
            holder.join(),
            reader.join(),
        )
    });
    assert!(locked.is_ok() && entered.is_ok() && released.is_ok());
    assert!(matches!(holder, Ok((true, true))) && matches!(reader, Ok((true, true))));
    assert!(
        waited,
        "collector probe must not turn a held read collection into final Unavailable"
    );
    let (allowance, stamp) = result.unwrap().unwrap();
    let allowance = allowance.unwrap();
    assert_eq!(allowance.expires_at_ms, 1_800_007);
    assert_eq!(
        allowance.restrictions,
        ExecutionRestrictions::unrestricted()
    );
    stamp.check_current().unwrap();
    let mut output = Vec::new();
    policy
        .originals
        .review_observations()
        .drain()
        .write_to(&mut output);
    assert!(
        output.is_empty(),
        "successful contention resolution emits no refusal"
    );
    policy
        .lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
}

#[tokio::test]
async fn admitted_work_collector_failures_keep_fixed_label_and_original_refusal() {
    let (_directory, source, policy, association, read) = collector_work_read_fixture().await;
    // Controller purpose is still denied for an enrolled operation collector.
    denied(policy.authorize_admitted_work(&association, &read, LocalPolicyPurpose::Controller, 7));
    let poisoned = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _guard = source.lock_originals_for_test();
                panic!("synthetic existing source-owner unwind");
            })
            .join()
    });
    assert!(poisoned.is_err());
    assert!(matches!(
        policy.authorize_admitted_work(&association, &read, LocalPolicyPurpose::Operation, 7),
        Err(OperationAuthorizationError::Unavailable)
    ));
    let mut output = Vec::new();
    policy
        .originals
        .review_observations()
        .drain()
        .write_to(&mut output);
    let rows = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            serde_json::json!({"event": "toolkit_installed_operation_denied", "stage": "work.collector", "cause": "Denied"}),
            serde_json::json!({"event": "toolkit_installed_operation_unavailable", "stage": "work.collector", "cause": "Unavailable"}),
        ]
    );
    policy
        .lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
}
