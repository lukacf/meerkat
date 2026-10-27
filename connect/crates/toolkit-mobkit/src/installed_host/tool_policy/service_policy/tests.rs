//! Real SQLite and the same native provider. Faults below affect only a named
//! Store acknowledgment; no agent, native task grant or live service is claimed.
use super::*;
use meerkat_core::{CompiledMemberToolGrants, PrincipalKind, TrustDomainId};
use meerkat_authorization_contracts::evidence::EvidenceId;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use toolkit_store::*;

struct FaultStore { inner: Arc<dyn ToolkitStore>, fault: AtomicUsize }
#[async_trait::async_trait]
impl ToolkitStore for FaultStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, r: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(r).await }
    async fn get(&self, k: &StoreKey) -> StoreResult<Option<StoredRecord>> { self.inner.get(k).await }
    async fn list_records(&self, s: &StoreScope, a: Option<&str>, n: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(s, a, n).await
    }
    async fn compare_and_swap(&self, t: Transition) -> StoreResult<StoredRecord> {
        let fault = self.fault.load(Ordering::SeqCst);
        let event = t.events.first().map(|event| event.kind.as_str());
        let selected = matches!((fault, event), (1 | 2, Some("accepted-current")) | (3, Some("native-refused")));
        if selected && self.fault.compare_exchange(fault, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            if fault == 2 { self.inner.compare_and_swap(t).await?; }
            return Err(StoreError::OwnerUnavailable);
        }
        self.inner.compare_and_swap(t).await
    }
    async fn claim_record(&self, k: &StoreKey, r: u64, w: &str, t: u64) -> StoreResult<RecordLease> {
        self.inner.claim_record(k, r, w, t).await
    }
    async fn renew_record_claim(&self, l: &RecordLease, t: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(l, t).await
    }
    async fn recover_record_claim(&self, l: &RecordLease) -> StoreResult<RecordLease> { self.inner.recover_record_claim(l).await }
    async fn release_record_claim(&self, l: &RecordLease) -> StoreResult<()> { self.inner.release_record_claim(l).await }
    async fn read_events(&self, k: &StoreKey, a: u64, n: u32) -> StoreResult<Vec<StoredEvent>> { self.inner.read_events(k, a, n).await }
    async fn get_effect(&self, k: &EffectKey) -> StoreResult<Option<StoredEffect>> { self.inner.get_effect(k).await }
    async fn list_effects(&self, k: &StoreKey, a: Option<&str>, n: u32) -> StoreResult<Vec<StoredEffect>> { self.inner.list_effects(k, a, n).await }
    async fn claim_effect(&self, k: &EffectKey, r: u64, w: &str, t: u64) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(k, r, w, t).await
    }
    async fn acknowledge_effect(&self, l: &EffectLease, o: EffectOutcome) -> StoreResult<StoredEffect> { self.inner.acknowledge_effect(l, o).await }
    async fn claim_reconciliation(&self, k: &EffectKey, w: &str, t: u64) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(k, w, t).await
    }
    async fn reconcile_effect(&self, l: &EffectLease, r: Reconciliation) -> StoreResult<StoredEffect> { self.inner.reconcile_effect(l, r).await }
}

struct ExactRule {
    owner: PrincipalRef,
    approved: Mutex<BTreeSet<String>>,
    current: Arc<AtomicBool>,
}
struct Reservation { owner: PrincipalRef, current: Arc<AtomicBool>, provenance: serde_json::Map<String, serde_json::Value> }
impl ApplicationServicePolicyReservation for Reservation {
    fn owner(&self) -> &PrincipalRef { &self.owner }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.current.load(Ordering::SeqCst) { Ok(()) } else { Err(HostLifetimeError::Denied) }
    }
}
impl ExactRule {
    fn approve(&self, proposal: &ServiceToolPolicyProposal) {
        self.approved.lock().unwrap().insert(proposal.sha256().unwrap());
    }
}
impl ApplicationServicePolicy for ExactRule {
    fn reserve(&self, _: &crate::private_originals::ApplicationServicePolicyRequest<'_>)
        -> Result<crate::private_originals::ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(crate::private_originals::ApplicationServicePolicyDecision::RequiresHuman)
    }
    fn reserve_tool_policy_recovery(&self, request: &ApplicationToolPolicyRecoveryRequest<'_>)
        -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        let decision = request.decision;
        if decision.owner != self.owner || decision.realm != "realm" || decision.deployment != "deployment"
            || request.canonical_json != decision.proposal.candidate_canonical_json.as_bytes()
            || decision.approval_id != decision.proposal.sha256()?
            || !self.approved.lock().unwrap().contains(&decision.approval_id)
            || !self.current.load(Ordering::SeqCst) { return Err(HostLifetimeError::Denied); }
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(Reservation {
            owner: self.owner.clone(), current: self.current.clone(),
            provenance: [("fixture".into(), "exact-whole-candidate".into())].into_iter().collect(),
        })))
    }
}
fn principal() -> PrincipalRef {
    PrincipalRef::in_domain(PrincipalKind::ServiceAccount, "application", TrustDomainId::new("policy-test").unwrap()).unwrap()
}
fn bootstrap() -> String {
    String::from_utf8(CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("application").unwrap(), PolicyId::new("selected").unwrap(), PolicyRevision(1),
        CompiledPolicySourceProvenance { source_id: "fixture".into(), source_digest: PolicyDigest::from_canonical_bytes(b"fixture") },
        vec![CompiledMemberToolGrants { member_identity: "original".into(), grants: vec![] },
            CompiledMemberToolGrants { member_identity: "other".into(), grants: vec![] }],
    ).unwrap().canonical_json().unwrap()).unwrap()
}
fn selection(name: &str) -> ServiceToolPolicySelection {
    ServiceToolPolicySelection { tools: vec![ServiceToolInvocation { name: name.into(), consequence: CompiledToolConsequence::R0 }], catalog: None }
}
fn original(label: &str) -> PrivateOriginalRef {
    PrivateOriginalRef { route: toolkit_package::Id::new("route").unwrap(),
        original: EvidenceId::new(label).unwrap(), event_sha256: sha(label.as_bytes()) }
}
fn acquisition(label: &str) -> StoreKey {
    // The same address emitted by SetupStore, not the administrative slot scope.
    StoreKey { realm: "realm".into(), owner: "toolkit/acquisition/deployment".into(), id: label.into() }
}
async fn fixture() -> (tempfile::TempDir, PreparedInstalledToolPolicy, Arc<ServicePolicyOwner>, Arc<ExactRule>, Arc<FaultStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FaultStore { inner: Arc::new(toolkit_store::SqliteStore::open(dir.path().join("state.sqlite3")).unwrap()),
        fault: AtomicUsize::new(0) });
    let policy = PreparedInstalledToolPolicy::from_canonical(bootstrap().as_bytes()).unwrap();
    let rule = Arc::new(ExactRule { owner: principal(), approved: Mutex::new(BTreeSet::new()), current: Arc::new(AtomicBool::new(true)) });
    let owner = policy.bind_service_owner(store.clone(), StoreScope { realm: "realm".into(), owner: "deployment".into() },
        principal(), Some(rule.clone())).await.unwrap();
    (dir, policy, owner, rule, store)
}
async fn prepared(owner: &Arc<ServicePolicyOwner>, rule: &ExactRule, task: &str, name: &str) -> PreparedServiceToolPolicy {
    let prepared = owner.prepare("original", selection(name), None, original(task), acquisition(task)).await.unwrap();
    rule.approve(&prepared.proposal);
    prepared
}

#[tokio::test]
async fn accepted_native_candidate_survives_failed_promotion_or_lost_store_ack_on_same_provider() {
    for fault in [1, 2] {
        let (_dir, policy, owner, rule, store) = fixture().await;
        let prepared = prepared(&owner, &rule, "request", "discovered_read").await;
        let candidate = prepared.proposal.candidate_canonical_json.clone();
        let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
        let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
        store.fault.store(fault, Ordering::SeqCst);
        assert!(transaction.publish(&|| Ok(())).await.is_err());
        assert_eq!(transaction.observation().native, Some(ServiceToolPolicyNativeOutcome::Accepted));
        assert!(owner.check_current().is_err(), "failed promotion cannot leave prior readiness");
        drop(transaction);
        // This retained provider already accepted R2. Accepting R1 first would
        // fail with RevisionRollback; recovery must replay R2 directly.
        owner.recover().await.unwrap();
        assert!(owner.is_observed_current(candidate.as_bytes()).unwrap());
        let (_, slot) = owner.read().await.unwrap();
        assert!(slot.pending.is_none());
        assert_eq!(slot.current, candidate);
        assert_eq!(policy.compiled().canonical_json().unwrap(), bootstrap().as_bytes());
        assert!(policy.clone().successor(bootstrap().as_bytes(), candidate.as_bytes()).is_ok());
        assert!(policy.clone().successor(candidate.as_bytes(), bootstrap().as_bytes()).is_err());
    }
}

#[tokio::test]
async fn active_reservation_revocation_blocks_admission_and_recovery() {
    let (_dir, _policy, owner, rule, _store) = fixture().await;
    let prepared = prepared(&owner, &rule, "request", "discovered_read").await;
    let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
    let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
    transaction.publish(&|| Ok(())).await.unwrap(); drop(transaction);
    rule.current.store(false, Ordering::SeqCst);
    assert!(owner.check_current().is_err());
    assert!(owner.observed_current.lock().unwrap().is_none());
    assert!(owner.recover().await.is_err());
    assert!(owner.observed_current.lock().unwrap().is_none());
}

#[tokio::test]
async fn approved_task_replay_reuses_exact_candidate_and_cannot_change_selection() {
    let (_dir, _policy, owner, rule, _store) = fixture().await;
    let prepared = prepared(&owner, &rule, "request", "discovered_read").await;
    let proposal = prepared.proposal.clone();
    let approval = ServiceToolPolicyApproval::until_superseded(&proposal).unwrap();
    let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
    let first = transaction.publish(&|| Ok(())).await.unwrap(); drop(transaction);
    let replay = owner.prepare("original", selection("discovered_read"), None, original("request"), acquisition("request")).await.unwrap();
    assert_eq!(replay.proposal, proposal);
    let mut replay = replay.begin(&approval, &|| Ok(())).await.unwrap();
    let repeated = replay.publish(&|| Ok(())).await.unwrap(); drop(replay);
    assert_eq!(first.revision, repeated.revision);
    assert!(owner.prepare("original", selection("other_read"), None, original("request"), acquisition("request")).await.is_err());
}

#[tokio::test]
async fn retained_known_refusal_settles_before_any_candidate_replay() {
    // State-transition control for a surviving owner's known return. It does
    // not simulate or claim a native refusal. The actual provider remains R1.
    let (_dir, _policy, owner, rule, store) = fixture().await;
    let prepared = prepared(&owner, &rule, "request", "discovered_read").await;
    let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
    let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
    store.fault.store(3, Ordering::SeqCst);
    assert!(owner.settle_refusal(&transaction.stored, &mut transaction.slot).await.is_err());
    assert!(owner.known_refusal.lock().unwrap().is_some());
    drop(transaction);
    owner.recover().await.unwrap();
    let (_, slot) = owner.read().await.unwrap();
    assert!(slot.pending.is_none()); assert_eq!(slot.current, bootstrap());
    assert!(owner.known_refusal.lock().unwrap().is_none());
    assert!(owner.is_observed_current(bootstrap().as_bytes()).unwrap());
    owner.recover().await.unwrap();
    assert_eq!(owner.read().await.unwrap().1.current, bootstrap());
}

#[tokio::test]
async fn pending_foreign_slot_or_unselected_grants_are_refused_before_native_acceptance() {
    for foreign_slot in [true, false] {
        let (_dir, _policy, owner, rule, _store) = fixture().await;
        let prepared = prepared(&owner, &rule, "request", "discovered_read").await;
        let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
        let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
        let pending = transaction.slot.pending.as_mut().unwrap();
        let p = &mut pending.decision.proposal;
        let mut candidate = CompiledApplicationToolPolicy::parse_canonical_json(p.candidate_canonical_json.as_bytes()).unwrap();
        if foreign_slot { candidate.policy_id = PolicyId::new("foreign-slot").unwrap(); }
        else { candidate.members[1].grants.push(CompiledMemberToolGrant { tool_name: "not_selected".into(),
            action: CompiledMemberToolAction::Invoke, consequence: CompiledToolConsequence::R0 }); }
        let bytes = CompiledApplicationToolPolicy::new(candidate.provider_id, candidate.policy_id, candidate.revision,
            candidate.source, candidate.members).unwrap().canonical_json().unwrap();
        p.candidate_canonical_json = String::from_utf8(bytes.clone()).unwrap(); p.candidate = version(&bytes).unwrap();
        if foreign_slot { p.policy_id = PolicyId::new("foreign-slot").unwrap(); }
        pending.decision.approval_id = p.sha256().unwrap();
        pending.observation.proposal_sha256 = pending.decision.approval_id.clone(); pending.observation.candidate = p.candidate.clone();
        owner.change(&transaction.stored, &transaction.slot, "corrupt-fixture").await.unwrap();
        drop(transaction);
        assert!(owner.recover().await.is_err());
        // Equal bootstrap replay still succeeds: no R2 candidate was accepted.
        owner.provider.accept_canonical_json(bootstrap().as_bytes()).unwrap();
        assert!(owner.observed_current.lock().unwrap().is_none());
    }
}

#[test]
fn catalog_names_are_explicit_and_cannot_duplicate_selected_tools() {
    let mut selected = selection("read"); selected.catalog = Some(CompiledToolConsequence::R0);
    let grants = selected.grants().unwrap();
    assert_eq!(grants.iter().map(|grant| grant.tool_name.as_str()).collect::<Vec<_>>(),
        ["read", "tool_catalog_load", "tool_catalog_search"]);
    selected.tools.push(ServiceToolInvocation { name: "tool_catalog_load".into(), consequence: CompiledToolConsequence::R0 });
    assert!(selected.grants().is_err());
}

#[tokio::test]
async fn policy_decode_and_recovery_refuse_foreign_or_bare_acquisition_scopes() {
    for (field, value) in [("realm", "other-realm"), ("owner", "deployment"),
        ("owner", "toolkit/acquisition/other-deployment")] {
        let (_dir, _policy, owner, rule, _store) = fixture().await;
        let mut wrong = acquisition("request");
        match field { "realm" => wrong.realm = value.into(), _ => wrong.owner = value.into() };
        let prepared = prepared(&owner, &rule, "valid-request", "discovered_read").await;
        let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
        let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
        owner.decode(&transaction.stored).unwrap();
        let mut decision = transaction.slot.pending.as_ref().unwrap().decision.clone();
        decision.acquisition = wrong.clone();
        assert!(owner.decision(&decision, decision.proposal.candidate_canonical_json.as_bytes()).is_err(),
            "decision {field}={value}");
        let mut pending = transaction.slot.clone();
        pending.pending.as_mut().unwrap().acquisition = wrong.clone();
        let mut stored = transaction.stored.clone();
        stored.data = encode(&pending).unwrap();
        assert!(owner.decode(&stored).is_err(), "pending {field}={value}");
        transaction.publish(&|| Ok(())).await.unwrap(); drop(transaction);
        let (stored, mut current) = owner.read().await.unwrap();
        current.decision.as_mut().unwrap().acquisition = wrong;
        owner.change(&stored, &current, "corrupt-fixture").await.unwrap();
        assert!(owner.recover().await.is_err(), "current {field}={value}");
        assert!(owner.observed_current.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn legacy_service_permit_cannot_recover_or_publish_member_policy() {
    struct Legacy { owner: PrincipalRef }
    impl ApplicationServicePolicy for Legacy {
        fn reserve(&self, _: &crate::private_originals::ApplicationServicePolicyRequest<'_>)
            -> Result<crate::private_originals::ApplicationServicePolicyDecision, HostLifetimeError> {
            Ok(crate::private_originals::ApplicationServicePolicyDecision::Permit(Box::new(Reservation {
                owner: self.owner.clone(), current: Arc::new(AtomicBool::new(true)),
                provenance: [("fixture".into(), "legacy-service-only".into())].into_iter().collect(),
            })))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolkitStore> = Arc::new(SqliteStore::open(dir.path().join("state.sqlite3")).unwrap());
    let policy = PreparedInstalledToolPolicy::from_canonical(bootstrap().as_bytes()).unwrap();
    let owner = policy.bind_service_owner(store, StoreScope { realm: "realm".into(), owner: "deployment".into() },
        principal(), Some(Arc::new(Legacy { owner: principal() }))).await.unwrap();
    let prepared = owner.prepare("original", selection("discovered_read"), None, original("request"), acquisition("request")).await.unwrap();
    let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
    assert!(prepared.begin(&approval, &|| Ok(())).await.is_err(), "an acknowledgment value does not extend a legacy owner");
    let (_, slot) = owner.read().await.unwrap();
    assert!(slot.pending.is_none()); assert_eq!(slot.current, bootstrap());
}

#[tokio::test]
async fn stale_prepared_successor_refuses_after_exact_current_policy_advances() {
    let (_dir, _policy, owner, rule, store) = fixture().await;
    // Both requests are genuinely prepared and approved against the same
    // canonical slot. No prior, row or approval is rewritten for this check.
    let first = prepared(&owner, &rule, "first", "first_read").await;
    let stale = prepared(&owner, &rule, "second", "second_read").await;
    assert_eq!(first.proposal.prior, stale.proposal.prior);
    assert_eq!(first.prior, stale.prior);
    assert_ne!(first.proposal.candidate, stale.proposal.candidate);
    let candidate = first.proposal.candidate_canonical_json.clone();
    let policy_id = first.proposal.policy_id.clone();
    let first_approval = ServiceToolPolicyApproval::until_superseded(&first.proposal).unwrap();
    let stale_approval = ServiceToolPolicyApproval::until_superseded(&stale.proposal).unwrap();
    let stale_digest = stale.proposal.sha256().unwrap();
    let mut transaction = first.begin(&first_approval, &|| Ok(())).await.unwrap();
    transaction.publish(&|| Ok(())).await.unwrap();
    assert_eq!(transaction.observation().native, Some(ServiceToolPolicyNativeOutcome::Accepted));
    drop(transaction);

    let (before, slot) = owner.read().await.unwrap();
    assert!(slot.pending.is_none());
    assert_eq!(slot.current, candidate);
    assert_ne!(version(slot.current.as_bytes()).unwrap(), stale.proposal.prior);
    assert_eq!(stale.proposal.sha256().unwrap(), stale_digest);
    let events = store.read_events(&owner.key, 0, 32).await.unwrap();
    assert!(events.iter().any(|event| event.kind == "accepted-current"));
    let native = owner.provider.snapshot(&policy_id).unwrap();
    assert_eq!(native.provenance().revision, version(candidate.as_bytes()).unwrap().revision);

    assert!(matches!(stale.begin(&stale_approval, &|| Ok(())).await,
        Err(HostLifetimeError::Denied)));
    let (after, after_slot) = owner.read().await.unwrap();
    assert_eq!(after, before, "stale approved proposal cannot alter the canonical slot");
    assert_eq!(encode(&after_slot).unwrap(), encode(&slot).unwrap());
    assert_eq!(store.read_events(&owner.key, 0, 32).await.unwrap(), events);
    assert!(Arc::ptr_eq(&native, &owner.provider.snapshot(&policy_id).unwrap()),
        "refusal precedes any native provider acceptance");
    assert!(owner.is_observed_current(candidate.as_bytes()).unwrap());
}
