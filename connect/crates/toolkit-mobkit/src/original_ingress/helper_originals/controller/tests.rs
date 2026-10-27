//! Storage/currentness controls over actual SQLite and native grant owners.
//! WorkGraph admission is the existing source fixture, not a live dispatch.
use super::*;
use super::super::tests::{fixture, retained};
use meerkat_core::service::WorkGraphNamespaceGrant;
use meerkat_workgraph::SqliteWorkGraphStore;
use std::sync::atomic::{AtomicU8, Ordering};
use toolkit_store::*;

// Fault only returned storage settlement. Never replace a native policy owner.
// 1/2: refuse before / lose reply after intent admit; 3/4: same for final CAS.
struct FaultStore { inner: SqliteStore, fault: AtomicU8 }
#[async_trait::async_trait]
impl ToolkitStore for FaultStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        let mode = self.fault.swap(0, Ordering::SeqCst);
        if mode == 1 { return Err(StoreError::OwnerUnavailable); }
        let result = self.inner.admit(request).await?;
        if mode == 2 { return Err(StoreError::OwnerUnavailable); }
        Ok(result)
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> { self.inner.get(key).await }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredRecord>> { self.inner.list_records(scope, after, limit).await }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let mode = self.fault.swap(0, Ordering::SeqCst);
        if mode == 3 { return Err(StoreError::OwnerUnavailable); }
        let result = self.inner.compare_and_swap(transition).await?;
        if mode == 4 { return Err(StoreError::OwnerUnavailable); }
        Ok(result)
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        self.inner.claim_record(key, revision, worker, ttl).await
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    owner: Arc<ToolkitOriginalOwner>, store: FaultStore,
    admitted: AdmittedHelperWork, binding: HelperNativeBinding,
}
impl Fixture {
    async fn new() -> Self { Self::build(false).await }
    async fn build(report: bool) -> Self {
        Self::build_with_operations(report, false).await
    }
    async fn with_operations() -> Self {
        Self::build_with_operations(true, true).await
    }
    async fn build_with_operations(report: bool, operations: bool) -> Self {
        Self::build_with_ancestors(report, operations, false).await
    }
    async fn with_ancestor() -> Self {
        Self::build_with_ancestors(true, true, true).await
    }
    async fn with_account() -> Self { Self::build_full(true, true, true, true, false, false).await }
    async fn with_public(account: bool) -> Self { Self::build_full(true, true, true, account, true, false).await }
    async fn build_with_ancestors(report: bool, operations: bool, recursive: bool) -> Self {
        Self::build_full(report, operations, recursive, false, false, false).await
    }
    async fn with_catalog() -> Self { Self::build_full(true, true, true, true, true, true).await }
    async fn build_full(report: bool, operations: bool, recursive: bool, account: bool, public: bool, catalog: bool) -> Self {
        let (directory, owner, base) = fixture();
        let ancestors = if recursive {
            vec![retained(&owner, &base, "recorded-ancestor", "user", &[])]
        } else { vec![] };
        let parent = retained(&owner, &base, "controller-parent", "user", &ancestors);
        // Deliberately do not put the designated source first. It is selected
        // by the existing owner argument, never by recursive traversal order.
        let mut contributors = ancestors;
        contributors.push(parent.clone());
        let work = WorkGraphService::with_namespace_grant(
            Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite3")).unwrap()),
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        ).unwrap();
        let report = if report {
            let mut bounds = ExecutionRestrictions::unrestricted();
            bounds.actions = meerkat_authorization_contracts::constraints::ExactRestriction::exact([
                meerkat_authorization_contracts::constraints::ActionRef { feature: "toolkit".into(), action: "setup-report".into() },
            ]);
            bounds.delegation_depth = DelegationDepth::remaining(1);
            let grant = owner.grants.issue_root(
                &crate::original_ingress::tests::principal("admin"), EvidenceId::new("independent-report-parent").unwrap(),
                crate::original_ingress::tests::principal("agent"), None, bounds,
            ).unwrap();
            let account_parent = if account {
                let mut bounds = ExecutionRestrictions::unrestricted();
                bounds.actions = meerkat_authorization_contracts::constraints::ExactRestriction::exact([
                    meerkat_authorization_contracts::constraints::ActionRef {
                        feature: "toolkit".into(), action: "setup-account-request".into(),
                    },
                ]);
                bounds.delegation_depth = DelegationDepth::remaining(1);
                Some(vec![owner.grants.issue_root(
                    &crate::original_ingress::tests::principal("admin"),
                    EvidenceId::new("independent-account-parent").unwrap(),
                    crate::original_ingress::tests::principal("agent"), None, bounds,
                ).unwrap()])
            } else { None };
            let public_parent = if public {
                let mut bounds = ExecutionRestrictions::unrestricted();
                bounds.actions = meerkat_authorization_contracts::constraints::ExactRestriction::exact([
                    meerkat_authorization_contracts::constraints::ActionRef {
                        feature: "toolkit".into(), action: "setup-public-service-request".into(),
                    },
                ]);
                bounds.delegation_depth = DelegationDepth::remaining(1);
                Some(vec![owner.grants.issue_root(
                    &crate::original_ingress::tests::principal("admin"),
                    EvidenceId::new("independent-public-parent").unwrap(),
                    crate::original_ingress::tests::principal("agent"), None, bounds,
                ).unwrap()])
            } else { None };
            Some(crate::installed_host::acquisition::ConfiguredHelperReportCapability { parent: vec![grant], account_parent, public_parent })
        } else { None };
        let mut operation_basis = if operations {
            use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
            let issue = |id: &str, action: &str| {
                let mut bounds = ExecutionRestrictions::unrestricted();
                bounds.actions = meerkat_authorization_contracts::constraints::ExactRestriction::exact([
                    meerkat_authorization_contracts::constraints::ActionRef {
                        feature: "toolkit".into(), action: action.into(),
                    },
                ]);
                bounds.delegation_depth = DelegationDepth::remaining(0);
                owner.grants.issue_root(&crate::original_ingress::tests::principal("admin"),
                    EvidenceId::new(id).unwrap(), crate::original_ingress::tests::principal("setup-agent"),
                    None, bounds).unwrap()
            };
            Some(WorkAuthorityBasis::TargetedGrantLineages {
                base: Some(vec![issue("independent-catalog", "catalog-read")]),
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer { server: "existing-server".into() },
                    lineage: vec![issue("independent-mcp", "mcp-call")],
                }],
            })
        } else { None };
        let catalog = if catalog {
            let parent = owner.grants.issue_root(&crate::original_ingress::tests::principal("admin"),
                EvidenceId::new("independent-successor-catalog-parent").unwrap(),
                crate::original_ingress::tests::principal("agent"), None,
                crate::installed_host::application_operations::configured_catalog_parent(
                    &crate::original_ingress::tests::principal("agent"),
                    &crate::original_ingress::tests::principal("setup-agent"),
                    &parent.association().candidate().target.audience),
            ).unwrap();
            let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = operation_basis.as_mut().unwrap() else { unreachable!() };
            *base = None;
            Some(crate::installed_host::acquisition::ConfiguredHelperCatalogCapability { parent: vec![parent] })
        } else { None };
        let (admitted, binding) = super::super::tests::admitted_source_fixture_with_report_basis(
            &owner, &work, &parent, contributors, &base, report, operation_basis, catalog,
        ).await;
        let store = FaultStore { inner: SqliteStore::open(&directory.path().join("sources.sqlite3")).unwrap(),
            fault: AtomicU8::new(0) };
        Self { _directory: directory, owner, store, admitted, binding }
    }
    fn candidate(&self) -> HelperControllerIntent {
        let mut requested = self.binding.controller_ceiling.clone();
        requested.delegation_depth = DelegationDepth::remaining(0);
        HelperControllerIntent::new(&self.admitted, self.binding.target.clone(),
            self.binding.controller_model.clone(), requested).unwrap()
    }
    async fn row(&self) -> Option<StoredRecord> {
        self.store.get(&self.admitted.source_key().unwrap()).await.unwrap()
    }
    fn assert_no_source(&self) {
        let original = self.admitted.original_work().unwrap();
        let mut namespace = self.admitted.contributor.association().candidate().ingress_namespace.clone();
        namespace.occurrence_scope = original.work.clone();
        assert!(self.owner.original_disposition(&namespace, &original).is_err());
        assert!(self.owner.admitted_original_basis(&namespace, &original).unwrap().is_none());
    }
}

#[tokio::test]
async fn pending_intent_freezes_route_and_complete_intent_without_source_custody() {
    let fixture = Fixture::new().await;
    let candidate = fixture.candidate();
    let intent = fixture.owner.retain_helper_controller_intent(
        &fixture.store, &fixture.admitted, candidate.clone(),
    ).await.unwrap();
    let saved = fixture.row().await.unwrap();
    assert_eq!(saved.revision, 1);
    assert!(HelperSourceRecord::decode(&saved).unwrap().bound().is_err());
    assert!(fixture.owner.observe_helper_binding(&fixture.store, &fixture.admitted).await.unwrap().is_none());
    fixture.assert_no_source();
    // Exact replay retains all native fields, not only the deterministic ID.
    assert_eq!(fixture.owner.retain_helper_controller_intent(
        &fixture.store, &fixture.admitted, candidate.clone(),
    ).await.unwrap(), intent);
    let mut changed_route = candidate.clone();
    changed_route.controller_model = meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity { model: "changed-model".into(),
            provider: meerkat_core::Provider::Anthropic, self_hosted_server_id: None,
            provider_params: None, auth_binding: None },
        serde_json::from_value(serde_json::json!({"realm":"test-realm","account":"changed-account"})).unwrap(),
        "changed-provider".into(), "changed-route".into(),
    );
    let mut changed_target = candidate.clone();
    changed_target.target.context = EvidenceId::new("different-real-session").unwrap();
    let mut changed_bounds = candidate;
    changed_bounds.requested.audiences =
        meerkat_authorization_contracts::constraints::ExactRestriction::exact([
            fixture.binding.target.audience.clone(),
        ]);
    for changed in [changed_route, changed_target, changed_bounds] {
        assert!(matches!(fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, changed,
        ).await, Err(OriginalWriterError::Conflict)));
        assert_eq!(fixture.row().await.unwrap(), saved);
        assert!(fixture.owner.grants.issued_reference(&intent.id).unwrap().is_none());
    }
}

#[tokio::test]
async fn intent_write_failure_or_lost_reply_never_authorizes_issue() {
    for fault in [1, 2] {
        let fixture = Fixture::new().await;
        let candidate = fixture.candidate();
        fixture.store.fault.store(fault, Ordering::SeqCst);
        let staged = fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, candidate.clone(),
        ).await;
        assert!(matches!(staged, Err(OriginalWriterError::Unavailable)));
        assert!(fixture.owner.grants.issued_reference(&candidate.id).unwrap().is_none());
        fixture.assert_no_source();
        assert_eq!(fixture.row().await.is_some(), fault == 2);
        let retry = fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, candidate.clone(),
        ).await.unwrap();
        assert_eq!(retry, candidate.native(&fixture.admitted.facts));
        assert!(fixture.owner.grants.issued_reference(&retry.id).unwrap().is_none());
    }
}

#[tokio::test]
async fn lost_native_or_binding_reply_recovers_same_intent_child_and_source() {
    for fault in [3, 4] {
        let fixture = Fixture::new().await;
        let candidate = fixture.candidate();
        let before_issue = fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, candidate.clone(),
        ).await.unwrap();
        // The real grant owner appends once. Deliberately discard its first
        // response before binding, like an unknown native response. This does
        // not simulate a torn native store or qualify the live facade.
        let binding = stage_fixture_controller(&fixture.owner, &fixture.store,
            &fixture.admitted, fixture.binding.clone()).await;
        let leaf = binding.controller_grant_lineage.last().unwrap().clone();
        let recovered_intent = fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, candidate.clone(),
        ).await.unwrap();
        assert_eq!(recovered_intent, before_issue);
        assert_eq!(fixture.owner.grants.issued_reference(&before_issue.id).unwrap(), Some(leaf.clone()));
        fixture.store.fault.store(fault, Ordering::SeqCst);
        assert!(matches!(fixture.owner.bind_admitted_helper(
            &fixture.store, &fixture.admitted, binding.clone(),
        ).await, Err(OriginalWriterError::Unavailable)));
        fixture.assert_no_source();
        assert_eq!(fixture.row().await.unwrap().revision, if fault == 4 { 2 } else { 1 });
        let recovered = stage_fixture_controller(&fixture.owner, &fixture.store,
            &fixture.admitted, fixture.binding.clone()).await;
        assert_eq!(recovered.controller_grant_lineage, binding.controller_grant_lineage);
        let source = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, recovered).await.unwrap();
        let replay = fixture.owner.bind_admitted_helper(&fixture.store, &fixture.admitted, binding).await.unwrap();
        assert_eq!(source.source_record.revision, 2);
        assert_eq!(source.source_record, replay.source_record);
        assert!(Arc::ptr_eq(&source.original, &replay.original));
        assert_eq!(fixture.owner.grants.issued_reference(&before_issue.id).unwrap(), Some(leaf));
        assert_eq!(fixture.owner.retain_helper_controller_intent(
            &fixture.store, &fixture.admitted, candidate,
        ).await.unwrap(), before_issue);
    }
}

#[tokio::test]
async fn pending_intent_cannot_outlive_current_contributor_cancellation() {
    let fixture = Fixture::new().await;
    let candidate = fixture.candidate();
    fixture.owner.retain_helper_controller_intent(
        &fixture.store, &fixture.admitted, candidate.clone(),
    ).await.unwrap();
    let saved = fixture.row().await.unwrap();
    fixture.owner.originals.lock().unwrap().iter_mut().filter_map(OriginalRecord::bound_mut)
        .find(|held| Arc::ptr_eq(&held.retained, &fixture.admitted.contributor)).unwrap()
        .operation = OriginalOperationDisposition::CancellationPending;
    assert!(matches!(fixture.owner.retain_helper_controller_intent(
        &fixture.store, &fixture.admitted, candidate.clone(),
    ).await, Err(OriginalWriterError::Unavailable)));
    assert!(fixture.owner.observe_helper_binding(&fixture.store, &fixture.admitted).await.is_err());
    assert_eq!(fixture.row().await.unwrap(), saved);
    assert!(fixture.owner.grants.issued_reference(&candidate.id).unwrap().is_none());
    fixture.assert_no_source();
}

mod report_tests;
mod origin_tests;

mod account_tests;

mod catalog_tests;
