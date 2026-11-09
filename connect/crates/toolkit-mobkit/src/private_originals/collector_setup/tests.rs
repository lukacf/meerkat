use super::*;

pub(crate) fn fixture() -> (McpServerConfig, ConnectorAuthStatus, CollectorInstallation) {
    let mut config = McpServerConfig::stdio("gmail", "not-executed", vec![], Default::default());
    config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: "work".into(),
        slot_id: "gmail-owner".into(),
        issuer: "https://accounts.google.com".into(),
        client: "client".into(),
        resource: "https://gmail.googleapis.com".into(),
        strategy_id: "google-userinfo-v1".into(),
        account: "verified-subject".into(),
        operations: ["history_page", "get_profile", "get_message"]
            .into_iter()
            .map(|operation| {
                (
                    operation.into(),
                    meerkat_core::mcp_config::McpConnectorOperation {
                        required_scopes: ["gmail.readonly".into()].into(),
                        destination_origin: "https://gmail.googleapis.com".into(),
                        read_only: true,
                    },
                )
            })
            .collect(),
    });
    let status = ConnectorAuthStatus {
        slot: CredentialAccountRef {
            realm: RealmId::parse("work").unwrap(),
            account: CredentialAccountId::parse("gmail-owner").unwrap(),
        },
        phase: ConnectorAuthPhase::Authorized,
        verified_account: Some(meerkat::ConnectorVerifiedAccount {
            issuer: "https://accounts.google.com".into(),
            strategy_id: "google-userinfo-v1".into(),
            subject: "verified-subject".into(),
        }),
        scopes: vec!["gmail.readonly".into()],
        scope_evidence: None,
        expires_at: None,
        has_refresh_token: true,
    };
    let selection = CollectorInstallation::from_native(
        &config,
        &status,
        ["history_page", "get_profile", "get_message"]
            .into_iter()
            .map(String::from)
            .collect(),
    )
    .unwrap();
    (config, status, selection)
}

#[test]
fn own_selected_server_survives_unrelated_configuration_changes() {
    let (config, _, selection) = fixture();
    let unrelated =
        McpServerConfig::stdio("calendar", "changed-binary", vec![], Default::default());
    assert!(selection.select(&[unrelated, config.clone()]).is_ok());
    assert!(selection.select(&[config]).is_ok());
}

#[test]
fn missing_duplicate_or_changed_own_installation_is_not_current() {
    let (config, _, selection) = fixture();
    assert!(matches!(
        selection.select(&[]),
        Err(CollectorSetupError::InstallationChanged)
    ));
    assert!(matches!(
        selection.select(&[config.clone(), config.clone()]),
        Err(CollectorSetupError::InstallationChanged)
    ));
    let mut changed = config.clone();
    changed.connector.as_mut().unwrap().client = "other-client".into();
    assert!(matches!(
        selection.select(&[changed]),
        Err(CollectorSetupError::InstallationChanged)
    ));
    let mut changed = config;
    changed.connect_timeout_secs = Some(321);
    assert!(matches!(
        selection.select(&[changed]),
        Err(CollectorSetupError::InstallationChanged)
    ));
}

#[test]
fn current_installation_realm_fences_account_selection_and_recheck() {
    let (_, _, mut selection) = fixture();
    let credential_realm = RealmId::parse("work").unwrap();
    assert!(selection.check_credential_realm(&credential_realm).is_ok());
    assert_eq!(
        selection.check_credential_realm(&RealmId::parse("another").unwrap()),
        Err(CollectorSetupError::Denied)
    );
    selection.slot.realm = RealmId::parse("another").unwrap();
    assert_eq!(
        selection.check_credential_realm(&credential_realm),
        Err(CollectorSetupError::Denied)
    );
}

#[test]
fn native_verified_account_required_and_exact() {
    let (_, status, selection) = fixture();
    assert!(selection.check_account(&status).is_ok());
    let mut absent = status.clone();
    absent.verified_account = None;
    assert_eq!(
        selection.check_account(&absent),
        Err(CollectorSetupError::ReauthRequired)
    );
    let mut logged_out = status.clone();
    logged_out.phase = ConnectorAuthPhase::AuthorizationRequired;
    assert_eq!(
        selection.check_account(&logged_out),
        Err(CollectorSetupError::ReauthRequired)
    );
    for field in ["issuer", "strategy", "subject", "slot"] {
        let mut changed = status.clone();
        match field {
            "issuer" => {
                changed.verified_account.as_mut().unwrap().issuer = "https://other.invalid".into()
            }
            "strategy" => changed.verified_account.as_mut().unwrap().strategy_id = "other".into(),
            "subject" => changed.verified_account.as_mut().unwrap().subject = "other".into(),
            _ => changed.slot.account = CredentialAccountId::parse("another-slot").unwrap(),
        }
        assert_eq!(
            selection.check_account(&changed),
            Err(CollectorSetupError::PrincipalChanged)
        );
    }
}

#[test]
fn undeclared_write_and_unknown_operations_cannot_be_enrolled() {
    let (mut config, status, _) = fixture();
    assert!(matches!(
        CollectorInstallation::from_native(&config, &status, ["send_email".into()].into()),
        Err(CollectorSetupError::Denied)
    ));
    config
        .connector
        .as_mut()
        .unwrap()
        .operations
        .get_mut("history_page")
        .unwrap()
        .read_only = false;
    assert!(matches!(
        CollectorInstallation::from_native(&config, &status, ["history_page".into()].into()),
        Err(CollectorSetupError::Denied)
    ));
    assert!(matches!(
        CollectorInstallation::from_native(&config, &status, BTreeSet::new()),
        Err(CollectorSetupError::Denied)
    ));
}

#[test]
fn persisted_installation_has_no_executable_configuration_or_token() {
    let (mut config, status, _) = fixture();
    if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut config.transport {
        stdio
            .env
            .insert("HOST_PRIVATE".into(), "must-not-be-persisted".into());
    }
    let selection =
        CollectorInstallation::from_native(&config, &status, ["history_page".into()].into())
            .unwrap();
    let serialized = serde_json::to_string(&selection).unwrap();
    assert!(!serialized.contains("must-not-be-persisted"));
    assert!(!serialized.contains("HOST_PRIVATE"));
    assert!(!serialized.contains("not-executed"));
    assert!(selection.select(&[config]).is_ok());
}

#[test]
fn actual_operation_account_requires_selected_issuer_and_subject() {
    let (_, _, selection) = fixture();
    let account = meerkat_core::OperationAccount::Selected(
        meerkat_core::QualifiedAccount::new("https://accounts.google.com", "verified-subject")
            .unwrap(),
    );
    assert!(check_operation_account(&account, &selection.account).is_ok());
    assert_eq!(
        check_operation_account(
            &meerkat_core::OperationAccount::Unavailable,
            &selection.account
        ),
        Err(CollectorSetupError::PrincipalChanged)
    );
    assert_eq!(
        check_operation_account(
            &meerkat_core::OperationAccount::DeclaredNone,
            &selection.account
        ),
        Err(CollectorSetupError::PrincipalChanged)
    );
}

pub(crate) fn record(owner: &ToolkitOriginalOwner, grantee: &PrincipalRef) -> CollectorRecord {
    use crate::original_ingress::tests::{claims, principal};
    let (config, _, installation) = fixture();
    let audience = meerkat_authorization_contracts::constraints::AudienceRef::Principal {
        principal: principal("user"),
    };
    let mut restrictions = crate::installed_host::application_operations::mcp_access_restrictions(
        &owner.ingress_domain().authority,
        grantee,
        &audience,
        "gmail",
        installation.operations.iter().map(String::as_str),
        &config.connector_approval(),
    );
    restrictions.delegation_depth =
        meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1);
    let grant = owner
        .grants()
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("admin"),
            EvidenceId::new("collector-grant").unwrap(),
            grantee.clone(),
            None,
            restrictions,
        )
        .unwrap();
    let mut candidate = claims("connect-setup-native", &grant).candidate().clone();
    candidate.requester = principal("admin");
    candidate.ingress_actor = principal("admin");
    candidate.logical_executor = grantee.clone();
    candidate.ingress_namespace.realm = RealmId::parse("work").unwrap();
    candidate.target.logical_owner = owner.ingress_domain().authority.clone();
    candidate.target.logical_runtime = EvidenceId::new("meta-source/demo").unwrap();
    candidate.target.context = EvidenceId::new("inbox").unwrap();
    candidate.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: vec![TargetGrantLineage {
            target: GrantTarget::McpServer {
                server: "gmail".into(),
            },
            lineage: vec![grant.clone()],
        }],
    };
    let mut record = CollectorRecord {
        id: candidate.original_work.work.clone(),
        installation,
        source: CollectorSource {
            mob_id: EvidenceId::new("demo").unwrap(),
            source_id: EvidenceId::new("inbox").unwrap(),
            generation: 1,
        },
        association: candidate,
        lineage: vec![grant],
        operator: principal("admin"),
        revoked: false,
        review: None,
    };
    approve(&mut record);
    record
}

fn approve(record: &mut CollectorRecord) {
    use meerkat_core::*;
    record.review = None;
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("collector-finalizer").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: SessionId::new(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("installed-collector"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Enroll collector".into(),
            body: Some(record.review_body()),
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: None,
        metadata: Default::default(),
        request_provenance: None,
    };
    let approvals = ApprovalService::new();
    let pending = approvals.request(request).unwrap();
    record.review = Some(
        approvals
            .decide(
                &pending.approval_id,
                ApprovalDecision::Approve,
                ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(),
                None,
                None,
            )
            .unwrap(),
    );
}

#[test]
fn agent_parent_cannot_be_reinterpreted_as_collector_authority() {
    use crate::original_ingress::tests::{initialized, principal};
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants.jsonl"));
    let mut record = record(&owner, &principal("agent"));
    let parent = record.lineage[0].clone();
    let child = owner
        .grants()
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            &principal("agent"),
            &parent,
            EvidenceId::new("collector-child").unwrap(),
            principal("collector"),
            meerkat_authorization_contracts::constraints::ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    record.lineage.push(child);
    record.association.logical_executor = principal("collector");
    // Native delegation can exist, but the collector contract deliberately
    // requires operator-issued authority for the collector itself.
    assert!(
        owner
            .grants()
            .resolve_lineage(&record.lineage, &principal("collector"), None)
            .is_ok()
    );
    assert_eq!(
        check_grant(&owner, &record),
        Err(CollectorSetupError::Denied)
    );
}

#[test]
fn same_grantee_child_retains_the_exact_read_only_grant() {
    use crate::original_ingress::tests::{initialized, principal};
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants.jsonl"));
    let mut record = record(&owner, &principal("collector"));
    assert!(check_grant(&owner, &record).is_ok());
    let child = owner
        .grants()
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            &principal("collector"),
            &record.lineage[0],
            EvidenceId::new("collector-child").unwrap(),
            principal("collector"),
            meerkat_authorization_contracts::constraints::ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    record.lineage.push(child);
    assert!(check_grant(&owner, &record).is_ok());
}

#[tokio::test]
async fn unconfirmed_collector_cancellation_fences_native_enrollment_without_claiming_cleanup() {
    use crate::original_ingress::tests::{initialized, principal};
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, RetainedCustody};
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants.jsonl"));
    let store: Arc<dyn ToolkitStore> = Arc::new(
        toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap(),
    );
    let scope = StoreScope {
        realm: "work".into(),
        owner: "application".into(),
    };
    let originals = PrivateOriginals::prepare(scope.clone(), store.clone(), owner.clone())
        .await
        .unwrap();
    owner
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    let record = record(&owner, &principal("collector"));
    originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    let key = collector_key(&scope, &record.id).unwrap();
    let before = store.get(&key).await.unwrap().unwrap();
    {
        let _writer = owner.source_writer().await;
        // This is the real source fence immediately before cancellation's
        // durable write. No store success or native revocation is invented.
        owner.fence_collector_original(&key).unwrap();
    }
    assert!(matches!(
        owner.enrolled_service_original(
            &record.association.ingress_namespace,
            &record.association.original_work
        ),
        Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)
    ));
    assert!(matches!(
        owner.original_disposition(
            &record.association.ingress_namespace,
            &record.association.original_work
        ),
        Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)
    ));
    assert!(store.get(&key).await.unwrap().unwrap() == before);
    assert!(check_grant(&owner, &record).is_ok());
    assert!(matches!(
        originals.persist_collector_setup(record.clone()).await,
        Err(CollectorSetupError::Unavailable)
    ));
    assert!(matches!(
        owner
            .retained_custody(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        RetainedCustody::NotRetained
    ));
    originals.revoke_collector_setup(&record.id).await.unwrap();
    assert!(matches!(
        owner
            .original_disposition(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        meerkat_runtime::meerkat_machine::OriginalDisposition::OperationCancelled { .. }
    ));
    // Source cancellation does not assert that the separate grant was revoked.
    assert!(check_grant(&owner, &record).is_ok());
}

#[tokio::test]
async fn durable_enrollment_reopens_without_setup_session_and_revoked_id_never_reopens() {
    use crate::original_ingress::tests::{initialized, principal};
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, RetainedCustody};
    let directory = tempfile::tempdir().unwrap();
    let grant_path = directory.path().join("grants.jsonl");
    let source_path = directory.path().join("originals.sqlite3");
    let scope = StoreScope {
        realm: "work".into(),
        owner: "application".into(),
    };
    let owner = initialized(&grant_path);
    let store: Arc<dyn ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(&source_path).unwrap());
    let originals = PrivateOriginals::prepare(scope.clone(), store.clone(), owner.clone())
        .await
        .unwrap();
    owner
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    let record = record(&owner, &principal("collector"));
    let ingress = owner.ingress_domain().clone();
    assert!(
        owner
            .enrolled_service_original(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap()
            .is_none()
    );
    let id = originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    assert_eq!(
        originals
            .persist_collector_setup(record.clone())
            .await
            .unwrap(),
        id
    );
    let selected = owner
        .enrolled_service_original(
            &record.association.ingress_namespace,
            &record.association.original_work,
        )
        .unwrap()
        .unwrap();
    assert!(selected.association().candidate() == &record.association);
    assert!(matches!(
        owner
            .original_disposition(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        meerkat_runtime::meerkat_machine::OriginalDisposition::Active
    ));
    let mut foreign = record.association.ingress_namespace.clone();
    foreign.occurrence_scope = EvidenceId::new("another-source").unwrap();
    assert!(
        owner
            .enrolled_service_original(&foreign, &record.association.original_work)
            .unwrap()
            .is_none()
    );
    let mut changed = record.association.clone();
    changed.target.context_generation += 1;
    assert!(
        owner
            .current_collector_record(
                &meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                    changed
                )
                .unwrap()
            )
            .is_err()
    );
    assert!(matches!(
        owner
            .retained_custody(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        RetainedCustody::NotRetained
    ));
    drop(originals);
    drop(store);
    drop(owner);

    let owner = Arc::new(
        ToolkitOriginalOwner::restore(
            Arc::new(meerkat_authorization::grants::FileGrantOwnerStore::new(
                &grant_path,
                meerkat_authorization::grants::GrantLogFence::NotAdmissionRead,
            )),
            Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
            ingress,
        )
        .unwrap(),
    );
    let store: Arc<dyn ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(&source_path).unwrap());
    let originals = PrivateOriginals::prepare(scope.clone(), store.clone(), owner.clone())
        .await
        .unwrap();
    owner
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    let key = collector_key(&scope, &id).unwrap();
    let saved = decode(&store.get(&key).await.unwrap().unwrap(), &key).unwrap();
    assert!(saved == record);
    assert!(check_grant(&owner, &saved).is_ok());
    // A cold owner cannot answer from a locator alone. The same reviewed
    // canonical row and actual native lineage must be revalidated first.
    assert!(
        owner
            .enrolled_service_original(
                &saved.association.ingress_namespace,
                &saved.association.original_work
            )
            .unwrap()
            .is_none()
    );
    let before = store.get(&key).await.unwrap().unwrap();
    originals
        .persist_collector_setup(saved.clone())
        .await
        .unwrap();
    assert!(store.get(&key).await.unwrap().unwrap() == before);
    assert!(
        owner
            .enrolled_service_original(
                &saved.association.ingress_namespace,
                &saved.association.original_work
            )
            .unwrap()
            .unwrap()
            .association()
            .candidate()
            == &saved.association
    );
    assert!(matches!(
        owner
            .retained_custody(
                &saved.association.ingress_namespace,
                &saved.association.original_work
            )
            .unwrap(),
        RetainedCustody::NotRetained
    ));
    originals.revoke_collector_setup(&id).await.unwrap();
    let revoked = decode(&store.get(&key).await.unwrap().unwrap(), &key).unwrap();
    assert!(revoked.revoked);
    assert!(
        owner
            .enrolled_service_original(
                &saved.association.ingress_namespace,
                &saved.association.original_work
            )
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        owner
            .original_disposition(
                &saved.association.ingress_namespace,
                &saved.association.original_work
            )
            .unwrap(),
        meerkat_runtime::meerkat_machine::OriginalDisposition::OperationCancelled { .. }
    ));
    assert!(matches!(
        originals.persist_collector_setup(record).await,
        Err(CollectorSetupError::Denied)
    ));
}

#[tokio::test]
async fn collector_enrollment_cannot_shadow_an_ordinary_original_cancellation() {
    use crate::original_ingress::tests::{initialized, principal, private_event, private_route};
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants.jsonl"));
    let store: Arc<dyn ToolkitStore> = Arc::new(
        toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap(),
    );
    let scope = StoreScope {
        realm: "work".into(),
        owner: "application".into(),
    };
    let mut route = private_route("route");
    route.realm = RealmId::parse("work").unwrap();
    let originals = PrivateOriginals::new(scope.clone(), store.clone(), owner.clone(), vec![route])
        .await
        .unwrap();
    owner
        .select_installed_pair_recovery(store.as_ref(), &scope)
        .unwrap();
    let record = record(&owner, &principal("collector"));
    originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    assert!(matches!(
        owner
            .original_disposition(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        OriginalDisposition::Active
    ));

    // The ordinary source and collector use distinct durable keys, even when
    // their original-work IDs coincide. Its real cancellation must win.
    let ordinary = originals
        .retain_event(private_event(record.id.as_str()))
        .await
        .unwrap();
    originals
        .record_operation_cancelled(&ordinary)
        .await
        .unwrap();
    let cancelled = owner
        .original_disposition(
            &record.association.ingress_namespace,
            &record.association.original_work,
        )
        .unwrap();
    assert!(matches!(
        &cancelled,
        OriginalDisposition::OperationCancelled { .. }
    ));
    assert!(
        owner
            .enrolled_service_original(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap()
            .is_none()
    );
    originals
        .persist_collector_setup(record.clone())
        .await
        .unwrap();
    assert_eq!(
        owner
            .original_disposition(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        cancelled
    );
    assert!(
        owner
            .enrolled_service_original(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap()
            .is_none()
    );
    let key = collector_key(&scope, &record.id).unwrap();
    assert!(
        !decode(&store.get(&key).await.unwrap().unwrap(), &key)
            .unwrap()
            .revoked
    );
    assert!(check_grant(&owner, &record).is_ok());
}
