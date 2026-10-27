//! Protocol fixtures use a trusted in-process owner, not a production runtime.
use super::*;
use rusqlite::params;
use std::sync::{Arc, Mutex, atomic::AtomicUsize};

#[tokio::test]
async fn deadline_recovery_pages_due_rows_without_authorizing_or_repeating_expiry() {
    let mut f = Fixture::new();
    let mut due = Vec::new();
    for correlation in ["due-one", "due-two", "due-three"] {
        let mut host = f.request.host().clone();
        host.correlation = id(correlation);
        f.request = validate_request(f.request.request().clone(), host, &f.authority,
            &f.context(200), &f.references).unwrap();
        due.push(f.admit().await);
    }
    let mut before = Vec::new();
    for snapshot in &due { before.push(f.store.get(snapshot.key()).await.unwrap()); }
    // A valid future request in the same scope must not enter expiry work.
    let future_deadline = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 60_000;
    let mut constraints = f.authority.constraints().clone();
    constraints.expires_at_ms = future_deadline;
    f.authority = SetupRequestEvidence::new(f.authority.association().clone(), constraints).unwrap();
    let mut host = f.request.host().clone();
    host.correlation = id("future");
    host.limits.deadline_ms = future_deadline;
    f.request = validate_request(f.request.request().clone(), host, &f.authority,
        &f.context(200), &f.references).unwrap();
    let future = f.admit().await;
    let mut after = None;
    let mut observed = Vec::new();
    loop {
        let page = f.bridge().continuation_recovery_page(after.as_deref(), 1).await.unwrap();
        assert!(page.operations.is_empty());
        assert!(!page.had_unavailable_records);
        observed.extend(page.expiry_operations);
        after = page.next_after_id;
        if after.is_none() { break; }
    }
    let mut expected: Vec<_> = due.iter().map(|s| id(&s.key().id)).collect();
    expected.sort();
    assert_eq!(observed, expected);
    assert!(!observed.contains(&id(&future.key().id)));
    for (snapshot, previous) in due.iter().zip(before) {
        assert!(f.store.get(snapshot.key()).await.unwrap() == previous);
    }
    let foreign = SetupStore::new(&f.store, SetupScope::new(id("other"), f.deployment.clone())).unwrap();
    let page = foreign.continuation_recovery_page(None, 32).await.unwrap();
    assert!(page.expiry_operations.is_empty());
    let operation = id(&due[0].key().id);
    assert!(matches!(f.bridge().expire(&operation, &DeadlineOwner(false)).await, Err(SetupError::Denied)));
    // Race the existing canonical CAS, never renew the expired request token.
    let store = f.bridge();
    let (first, second) = tokio::join!(
        store.expire(&operation, &DeadlineOwner(true)),
        store.expire(&operation, &DeadlineOwner(true)),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let expired = first.or(second).unwrap();
    assert_eq!(expired.state().lifecycle_phase, AcquisitionPhase::Expired);
    let delivery = Delivery::new(0);
    assert!(matches!(step(&f, &expired, &delivery).await.unwrap(), ContinuationStep::Acknowledged(_)));
    let settled = f.store.get(expired.key()).await.unwrap();
    let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
    assert!(!page.expiry_operations.contains(&operation));
    assert!(!page.operations.contains(&operation));
    assert_eq!(delivery.dispatches.load(Ordering::SeqCst), 1);
    assert!(f.bridge().expire(&operation, &DeadlineOwner(true)).await.is_err());
    assert!(f.store.get(expired.key()).await.unwrap() == settled);
}

struct Delivery {
    admission: Mutex<Option<ContinuationAdmissionEvidence>>,
    delayed: Mutex<Option<ContinuationAdmissionEvidence>>,
    dispatches: AtomicUsize,
    reconciliations: AtomicUsize,
    observations: AtomicUsize,
    preflights: AtomicUsize,
    mode: AtomicUsize,
    reconciliation: AtomicUsize,
    authorized: AtomicBool,
    dispatch_authorized: AtomicBool,
    observation_authorized: AtomicBool,
    deny_observation_at: AtomicUsize,
    pause_preflight: AtomicBool,
    advance_observation: AtomicBool,
    preflight_started: tokio::sync::Notify,
    preflight_release: tokio::sync::Notify,
}

impl Delivery {
    fn new(mode: usize) -> Self {
        Self {
            admission: Mutex::new(None),
            delayed: Mutex::new(None),
            dispatches: 0.into(),
            reconciliations: 0.into(),
            observations: 0.into(),
            preflights: 0.into(),
            mode: mode.into(),
            reconciliation: 0.into(),
            authorized: AtomicBool::new(true),
            dispatch_authorized: AtomicBool::new(true),
            observation_authorized: AtomicBool::new(true),
            deny_observation_at: usize::MAX.into(),
            pause_preflight: AtomicBool::new(false),
            advance_observation: AtomicBool::new(false),
            preflight_started: tokio::sync::Notify::new(),
            preflight_release: tokio::sync::Notify::new(),
        }
    }
    fn evidence(obligation: &ContinuationObligation) -> ContinuationAdmissionEvidence {
        ContinuationAdmissionEvidence {
            receipt: continuation_receipt(),
            obligation_digest: obligation.digest().unwrap(),
            delivery_identity: obligation.delivery_identity().unwrap(),
            runtime: id("runtime-a"),
            session: id("session-a"),
            durable_input: reference("durable-input-a"),
            observed_at_ms: obligation.terminal_at_ms() + 1,
        }
    }
}
#[async_trait]
impl SetupContinuationOwner for Delivery {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        let call = self.observations.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.authorized.load(Ordering::SeqCst)
            || !self.observation_authorized.load(Ordering::SeqCst)
            || call >= self.deny_observation_at.load(Ordering::SeqCst)
        {
            return Err(ContinuationError::Denied);
        }
        let mut saved = self.admission.lock().unwrap().clone();
        if self.advance_observation.load(Ordering::SeqCst) {
            if let Some(saved) = &mut saved { saved.observed_at_ms += call as u64; }
        }
        if let Some(saved) = &saved {
            assert!(saved.receipt == *receipt);
            assert!(saved.obligation_digest == obligation.digest()?);
        }
        Ok(saved)
    }
}
#[async_trait]
impl SetupContinuationDelivery for Delivery {
    async fn authorize_delivery(
        &self,
        _obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        self.preflights.fetch_add(1, Ordering::SeqCst);
        if self.pause_preflight.load(Ordering::SeqCst) {
            self.preflight_started.notify_one();
            self.preflight_release.notified().await;
        }
        if self.authorized.load(Ordering::SeqCst) && self.dispatch_authorized.load(Ordering::SeqCst)
        {
            Ok(())
        } else {
            Err(ContinuationError::Denied)
        }
    }
    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        let mode = self.mode.load(Ordering::SeqCst);
        if !self.authorized.load(Ordering::SeqCst)
            || !self.dispatch_authorized.load(Ordering::SeqCst)
            || mode == 3
        {
            return Err(ContinuationError::Denied);
        }
        if mode == 2 {
            return Err(ContinuationError::OwnerUnavailable);
        }
        if mode == 4 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            return Err(ContinuationError::OwnerUnavailable);
        }
        let value = Self::evidence(obligation);
        if mode == 6 {
            *self.delayed.lock().unwrap() = Some(value);
            return Err(ContinuationError::OwnerUnavailable);
        }
        let mut saved = self.admission.lock().unwrap();
        if let Some(old) = &*saved {
            assert!(old == &value);
        } else {
            *saved = Some(value.clone());
        }
        if mode == 1 {
            return Err(ContinuationError::OwnerUnavailable);
        }
        if mode == 5 {
            return Ok(ContinuationAdmissionEvidence {
                session: id("forged-session"),
                ..value
            });
        }
        Ok(value)
    }
    async fn reconcile(
        &self,
        _obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        self.reconciliations.fetch_add(1, Ordering::SeqCst);
        if !self.authorized.load(Ordering::SeqCst)
            || !self.observation_authorized.load(Ordering::SeqCst)
        {
            return Err(ContinuationError::Denied);
        }
        let evidence = ContextReference {
            owner: id("runtime"),
            ..reference("reconciliation-proof")
        };
        Ok(match self.reconciliation.load(Ordering::SeqCst) {
            1 => ContinuationReconciliation::Pending { evidence },
            2 => ContinuationReconciliation::SafeToRetry { evidence },
            3 => ContinuationReconciliation::DefinitivelyRejected { evidence },
            4 => return Err(ContinuationError::Unsupported),
            _ => match self.admission.lock().unwrap().clone() {
                Some(evidence) => ContinuationReconciliation::Applied(evidence),
                None => ContinuationReconciliation::Pending { evidence },
            },
        })
    }
}
fn options() -> ContinuationStepOptions {
    ContinuationStepOptions {
        worker: id("executor-a"),
        lease_ms: 10000,
        owner_timeout_ms: 1000,
        settlement_budget_ms: 1000,
    }
}
fn effect_key(ended: &SetupSnapshot) -> EffectKey {
    EffectKey {
        record: ended.key().clone(),
        id: format!("continuation-{}", ended.state().terminal_revision),
    }
}
async fn step(
    f: &Fixture,
    ended: &SetupSnapshot,
    owner: &Delivery,
) -> Result<ContinuationStep, SetupError> {
    f.bridge()
        .step_continuation(&id(&ended.key().id), owner, &options())
        .await
}
async fn effect(f: &Fixture, ended: &SetupSnapshot) -> StoredEffect {
    f.store
        .get_effect(&effect_key(ended))
        .await
        .unwrap()
        .unwrap()
}
fn corrupt_effect(f: &Fixture, effect: &StoredEffect) {
    let connection = rusqlite::Connection::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    connection.execute("UPDATE toolkit_effects SET data=?1 WHERE realm=?2 AND owner=?3 AND record_id=?4 AND id=?5",
        params![serde_json::to_string(effect).unwrap(), effect.key.record.realm,
            effect.key.record.owner, effect.key.record.id, effect.key.id]).unwrap();
}

fn with_attempt_limit(mut f: Fixture, maximum: u16) -> Fixture {
    let mut host = f.request.host().clone();
    host.limits.max_effect_attempts = maximum;
    f.request = validate_request(
        f.request.request().clone(),
        host,
        &f.authority,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    f
}

// A protocol owner whose admission data is independently durable. A queued
// call can commit after the Toolkit caller loses its reply or stops dispatching.
// This is an exact-admission test owner, not a production runtime certificate.
struct DurableDelivery {
    path: std::path::PathBuf,
    queued: Mutex<Option<ContinuationAdmissionEvidence>>,
    dispatches: AtomicUsize,
    replay_safe: AtomicBool,
    dispatch_authorized: AtomicBool,
    observation_authorized: AtomicBool,
}

impl DurableDelivery {
    fn open(path: std::path::PathBuf) -> Self {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE IF NOT EXISTS admissions (identity TEXT PRIMARY KEY, receipt TEXT NOT NULL)").unwrap();
        Self {
            path,
            queued: Mutex::new(None),
            dispatches: 0.into(),
            replay_safe: AtomicBool::new(false),
            dispatch_authorized: AtomicBool::new(true),
            observation_authorized: AtomicBool::new(true),
        }
    }

    fn read(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        use rusqlite::OptionalExtension;
        if !self.observation_authorized.load(Ordering::SeqCst) {
            return Err(ContinuationError::Denied);
        }
        let connection = rusqlite::Connection::open(&self.path).unwrap();
        let encoded: Option<String> = connection
            .query_row(
                "SELECT receipt FROM admissions WHERE identity=?1",
                [obligation.delivery_identity()?.as_str()],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let value: Option<ContinuationAdmissionEvidence> =
            encoded.map(|value| serde_json::from_str(&value).unwrap());
        if let Some(value) = &value {
            assert!(value.obligation_digest == obligation.digest()?);
        }
        Ok(value)
    }

    fn commit_late(&self) {
        let evidence = self.queued.lock().unwrap().take().unwrap();
        let mut connection = rusqlite::Connection::open(&self.path).unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let encoded = serde_json::to_string(&evidence).unwrap();
        transaction
            .execute(
                "INSERT OR IGNORE INTO admissions(identity,receipt) VALUES(?1,?2)",
                params![evidence.delivery_identity.as_str(), encoded],
            )
            .unwrap();
        let stored: String = transaction
            .query_row(
                "SELECT receipt FROM admissions WHERE identity=?1",
                [evidence.delivery_identity.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, encoded);
        transaction.commit().unwrap();
    }
}

#[async_trait]
impl SetupContinuationOwner for DurableDelivery {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        let value = self.read(obligation)?;
        if let Some(value) = &value {
            assert!(value.receipt == *receipt);
        }
        Ok(value)
    }
}

#[async_trait]
impl SetupContinuationDelivery for DurableDelivery {
    async fn authorize_delivery(
        &self,
        _obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        if self.dispatch_authorized.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ContinuationError::Denied)
        }
    }

    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.authorize_delivery(obligation).await?;
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        *self.queued.lock().unwrap() = Some(Delivery::evidence(obligation));
        Err(ContinuationError::OwnerUnavailable)
    }

    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        if let Some(value) = self.read(obligation)? {
            return Ok(ContinuationReconciliation::Applied(value));
        }
        let evidence = ContextReference {
            owner: id("runtime"),
            ..reference("durable-exact-key-owner")
        };
        Ok(if self.replay_safe.load(Ordering::SeqCst) {
            ContinuationReconciliation::SafeToRetry { evidence }
        } else {
            ContinuationReconciliation::Pending { evidence }
        })
    }
}

#[tokio::test]
async fn late_commit_after_retry_safety_and_last_attempt_recovers_across_both_reopens() {
    for phase in [AcquisitionPhase::Cancelled, AcquisitionPhase::Expired] {
        let f = with_attempt_limit(Fixture::new(), 1);
        let ended = terminal(&f, phase).await;
        let operation = id(&ended.key().id);
        let owner_path = f.directory.path().join("owner.sqlite");
        let owner = DurableDelivery::open(owner_path.clone());
        assert!(
            f.bridge()
                .step_continuation(&operation, &owner, &options())
                .await
                .is_err()
        );
        owner.replay_safe.store(true, Ordering::SeqCst);
        assert!(matches!(
            f.bridge()
                .step_continuation(&operation, &owner, &options())
                .await
                .unwrap(),
            ContinuationStep::DispatchBudgetExhausted
        ));
        assert_eq!(effect(&f, &ended).await.status, EffectStatus::Pending);
        owner.dispatch_authorized.store(false, Ordering::SeqCst);
        owner.commit_late();
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
        drop(owner);
        let owner = DurableDelivery::open(owner_path);
        owner.dispatch_authorized.store(false, Ordering::SeqCst);
        // Old setup content access is gone, but the original delivery owner
        // still authorizes exact receipt observation after its durable reopen.
        f.references.0.store(false, Ordering::SeqCst);
        let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
        assert!(matches!(
            f.for_store(&reopened)
                .step_continuation(&operation, &owner, &options())
                .await
                .unwrap(),
            ContinuationStep::Acknowledged(_)
        ));
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 0);
        let settled = reopened
            .get_effect(&effect_key(&ended))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.attempt, 1);
        assert_eq!(settled.status, EffectStatus::Succeeded);
        let events = reopened.read_events(ended.key(), 0, 100).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.data.body["evidence"].get("RetrySafe").is_some())
        );
        assert!(
            !events
                .iter()
                .any(|event| event.data.body["evidence"].get("NotApplied").is_some())
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "toolkit.store.effect.claimed")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn retry_safety_then_dispatch_revocation_allows_only_current_observation() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(6);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.reconciliation.store(2, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::RetryAuthorized
    ));
    owner.dispatch_authorized.store(false, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Pending
    ));
    assert_eq!(effect(&f, &ended).await.attempt, 1);
    *owner.admission.lock().unwrap() = owner.delayed.lock().unwrap().take();
    owner.reconciliation.store(0, Ordering::SeqCst);
    owner.observation_authorized.store(false, Ordering::SeqCst);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Uncertain);
    assert_eq!(owner.observations.load(Ordering::SeqCst), 0);
    owner.observation_authorized.store(true, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(effect(&f, &ended).await.attempt, 1);
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_receipt_and_revoked_dispatch_never_invent_non_application_or_retry_safety() {
    let f = with_attempt_limit(Fixture::new(), 1);
    let ended = terminal(&f, AcquisitionPhase::Expired).await;
    let owner = Delivery::new(6);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.dispatch_authorized.store(false, Ordering::SeqCst);
    for _ in 0..3 {
        assert!(matches!(
            step(&f, &ended, &owner).await.unwrap(),
            ContinuationStep::DispatchBudgetExhausted
        ));
        let saved = effect(&f, &ended).await;
        assert_eq!(saved.attempt, 1);
        assert_eq!(saved.status, EffectStatus::Uncertain);
    }
    let events = f.store.read_events(ended.key(), 0, 100).await.unwrap();
    assert!(!events.iter().any(|event| {
        event.data.body["evidence"].get("RetrySafe").is_some()
            || event.data.body["evidence"].get("NotApplied").is_some()
    }));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    assert!(owner.delayed.lock().unwrap().is_some());
}

#[tokio::test]
async fn independently_acknowledged_pending_retry_is_repaired_without_counting_dispatch() {
    let f = with_attempt_limit(Fixture::new(), 1);
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let operation = id(&ended.key().id);
    let owner = Delivery::new(6);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.reconciliation.store(2, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::DispatchBudgetExhausted
    ));
    *owner.admission.lock().unwrap() = owner.delayed.lock().unwrap().take();
    owner.dispatch_authorized.store(false, Ordering::SeqCst);
    assert!(
        f.bridge()
            .acknowledge_continuation(&operation, &continuation_receipt(), &owner)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(effect(&f, &ended).await.attempt, 1);
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_preflight_cannot_exceed_attempt_budget_after_retry_safety() {
    let f = with_attempt_limit(Fixture::new(), 1);
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let paused = Arc::new(Delivery::new(0));
    paused.pause_preflight.store(true, Ordering::SeqCst);
    let operation = id(&ended.key().id);
    let bridge = f.bridge();
    let bounds = options();
    let waiting = bridge.step_continuation(&operation, paused.as_ref(), &bounds);
    let mut waiting = Box::pin(waiting);
    let mut entered = Box::pin(paused.preflight_started.notified());
    std::future::poll_fn(|context| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), context).is_pending(),
            "preflight must wait"
        );
        std::future::Future::poll(entered.as_mut(), context)
    })
    .await;
    let competitor = Delivery::new(2);
    assert!(step(&f, &ended, &competitor).await.is_err());
    competitor.reconciliation.store(2, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &competitor).await.unwrap(),
        ContinuationStep::DispatchBudgetExhausted
    ));
    paused.preflight_release.notify_one();
    assert!(matches!(
        waiting.await.unwrap(),
        ContinuationStep::DispatchBudgetExhausted
    ));
    assert_eq!(paused.dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(competitor.dispatches.load(Ordering::SeqCst), 1);
    let saved = effect(&f, &ended).await;
    // A granted claim consumes an attempt even when the returned count refuses
    // owner dispatch. Its uncertainty remains recoverable through observation.
    assert_eq!(saved.attempt, 2);
    assert_eq!(saved.status, EffectStatus::Uncertain);
    let before = saved.attempt;
    assert!(matches!(
        step(&f, &ended, &competitor).await.unwrap(),
        ContinuationStep::DispatchBudgetExhausted
    ));
    assert_eq!(effect(&f, &ended).await.attempt, before);
}

#[tokio::test]
async fn all_outcomes_settle_before_ack_and_reopen_without_another_admission() {
    for phase in [
        AcquisitionPhase::Ready,
        AcquisitionPhase::Unavailable,
        AcquisitionPhase::Failed,
        AcquisitionPhase::Cancelled,
        AcquisitionPhase::Expired,
    ] {
        let f = Fixture::new();
        let ended = terminal(&f, phase).await;
        let owner = Delivery::new(0);
        f.references.0.store(false, Ordering::SeqCst);
        assert!(matches!(
            step(&f, &ended, &owner).await.unwrap(),
            ContinuationStep::Acknowledged(_)
        ));
        let first = effect(&f, &ended).await;
        assert_eq!(first.status, EffectStatus::Succeeded);
        assert_eq!(first.attempt, 1);
        let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
        assert!(matches!(
            f.for_store(&reopened)
                .step_continuation(&id(&ended.key().id), &owner, &options())
                .await
                .unwrap(),
            ContinuationStep::Acknowledged(_)
        ));
        assert_eq!(first, effect(&f, &ended).await);
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
        let events = f.store.read_events(ended.key(), 0, 100).await.unwrap();
        let settled = events
            .iter()
            .position(|e| e.kind == "toolkit.store.effect.acknowledged")
            .unwrap();
        let acknowledged = events
            .iter()
            .position(|e| e.kind == "continuation_acknowledged")
            .unwrap();
        assert!(settled < acknowledged);
    }
}

#[tokio::test]
async fn lost_admission_reply_reconciles_exact_receipt_without_dispatch() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(1);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Uncertain);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(owner.reconciliations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn uncertainty_preserves_pending_and_requires_proven_retry_then_fresh_authority() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(2);
    assert!(step(&f, &ended, &owner).await.is_err());
    for _ in 0..3 {
        assert!(matches!(
            step(&f, &ended, &owner).await.unwrap(),
            ContinuationStep::Pending
        ));
        let current = effect(&f, &ended).await;
        assert_eq!(current.status, EffectStatus::Uncertain);
        assert!(current.lease.is_none());
        assert_eq!(current.attempt, 1);
    }
    owner.reconciliation.store(2, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::RetryAuthorized
    ));
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Pending);
    owner.authorized.store(false, Ordering::SeqCst);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    owner.authorized.store(true, Ordering::SeqCst);
    owner.mode.store(0, Ordering::SeqCst);
    // The denied observation retained uncertainty. Recover current owner
    // retry evidence before another authorized dispatch.
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::RetryAuthorized
    ));
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(effect(&f, &ended).await.attempt, 2);
}

#[tokio::test]
async fn actual_admission_denial_timeout_and_malformed_reply_remain_uncertain() {
    for mode in [3, 4, 5] {
        let f = Fixture::new();
        let ended = terminal(&f, AcquisitionPhase::Expired).await;
        let owner = Delivery::new(mode);
        let settings = ContinuationStepOptions {
            owner_timeout_ms: 5,
            ..options()
        };
        assert!(
            f.bridge()
                .step_continuation(&id(&ended.key().id), &owner, &settings)
                .await
                .is_err()
        );
        assert_eq!(effect(&f, &ended).await.status, EffectStatus::Uncertain);
        let record = f.store.get(ended.key()).await.unwrap().unwrap();
        assert_eq!(record.revision, ended.state().revision);
        owner.reconciliation.store(4, Ordering::SeqCst);
        assert!(step(&f, &ended, &owner).await.is_err());
        assert_eq!(effect(&f, &ended).await.status, EffectStatus::Uncertain);
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn settled_receipt_survives_ack_denial_and_next_step_cannot_resend() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(0);
    owner.deny_observation_at.store(2, Ordering::SeqCst);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Succeeded);
    assert_eq!(
        f.store.get(ended.key()).await.unwrap().unwrap().revision,
        ended.state().revision
    );
    owner
        .deny_observation_at
        .store(usize::MAX, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prior_acknowledgment_repairs_pending_outbox_without_delivery_permission() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(0);
    let observer = ContinuationOwner::new(0);
    f.bridge()
        .acknowledge_continuation(&id(&ended.key().id), &continuation_receipt(), &observer)
        .await
        .unwrap();
    let record = f.store.get(ended.key()).await.unwrap().unwrap();
    *owner.admission.lock().unwrap() =
        Some(serde_json::from_value(record.data.body["continuation_receipt"].clone()).unwrap());
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(owner.preflights.load(Ordering::SeqCst), 0);
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Succeeded);
}

#[tokio::test]
async fn corrupt_intents_are_rejected_before_any_owner_callback() {
    for mutation in 0..9 {
        let f = Fixture::new();
        let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
        let owner = Delivery::new(0);
        let mut saved = effect(&f, &ended).await;
        match mutation {
            0 => saved.intent.kind = "work_admission_requested".into(),
            1 => saved.intent.request.schema = "other_schema".into(),
            2 => saved.intent.request.version = 2,
            3 => saved.intent.request.body["continuation_id"] = "different-task".into(),
            4 => saved.intent.replay_safety = ReplaySafety::Idempotent,
            5 => saved.intent.idempotency_key = "different-key".into(),
            6 => saved.record_revision += 1,
            7 => saved.intent.id = "different-effect".into(),
            8 => saved.request_digest = "wrong-digest".into(),
            _ => unreachable!(),
        }
        if mutation != 8 {
            saved.request_digest = saved.intent.digest().unwrap();
        }
        corrupt_effect(&f, &saved);
        assert!(step(&f, &ended, &owner).await.is_err());
        assert_eq!(owner.preflights.load(Ordering::SeqCst), 0);
        assert_eq!(owner.observations.load(Ordering::SeqCst), 0);
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn altered_success_receipt_is_not_acknowledged_or_redispatched() {
    for field in ["runtime", "session", "durable_input"] {
        let f = Fixture::new();
        let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
        let owner = Delivery::new(0);
        owner.deny_observation_at.store(2, Ordering::SeqCst);
        assert!(step(&f, &ended, &owner).await.is_err());
        let mut saved = effect(&f, &ended).await;
        let Some(EffectOutcome::Succeeded { receipt }) = &mut saved.outcome else {
            panic!()
        };
        receipt.body[field] = if field == "durable_input" {
            serde_json::to_value(reference("forged-input")).unwrap()
        } else {
            "forged".into()
        };
        corrupt_effect(&f, &saved);
        owner
            .deny_observation_at
            .store(usize::MAX, Ordering::SeqCst);
        assert!(step(&f, &ended, &owner).await.is_err());
        assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.store.get(ended.key()).await.unwrap().unwrap().revision,
            ended.state().revision
        );
    }
}

#[tokio::test]
async fn dispatch_budget_does_not_block_last_attempt_reconciliation_or_receipt() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(2);
    owner.reconciliation.store(2, Ordering::SeqCst);
    for attempt in 1..=5 {
        assert!(step(&f, &ended, &owner).await.is_err());
        let result = step(&f, &ended, &owner).await.unwrap();
        if attempt == 5 {
            assert!(matches!(result, ContinuationStep::DispatchBudgetExhausted));
        } else {
            assert!(matches!(result, ContinuationStep::RetryAuthorized));
        }
    }
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::DispatchBudgetExhausted
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 5);
    assert_eq!(effect(&f, &ended).await.attempt, 5);

    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Expired).await;
    let owner = Delivery::new(1);
    let mut saved = effect(&f, &ended).await;
    saved.attempt = 4;
    corrupt_effect(&f, &saved);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert_eq!(effect(&f, &ended).await.attempt, 5);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
}

#[tokio::test]
async fn competing_workers_admit_at_most_once_and_converge_after_conflicts() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Arc::new(Delivery::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..12 {
        let store = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
        let scope = SetupScope::new(f.tenant.clone(), f.deployment.clone());
        let operation = id(&ended.key().id);
        let owner = owner.clone();
        tasks.spawn(async move {
            SetupStore::new(&store, scope)
                .unwrap()
                .step_continuation(
                    &operation,
                    owner.as_ref(),
                    &ContinuationStepOptions {
                        worker: id(&format!("worker-{n}")),
                        ..options()
                    },
                )
                .await
        });
    }
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            Ok(ContinuationStep::Acknowledged(_)) => (),
            Err(SetupError::Store(
                StoreError::Claimed | StoreError::Conflict | StoreError::EffectState,
            )) => (),
            _ => panic!("unexpected concurrency result"),
        }
    }
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(effect(&f, &ended).await.attempt, 1);
}

#[tokio::test]
async fn late_remote_call_and_safe_retry_share_exact_owner_admission() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(6);
    assert!(step(&f, &ended, &owner).await.is_err());
    assert!(owner.admission.lock().unwrap().is_none());
    // An empty lookup stays Pending while the original remote call is alive.
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Pending
    ));
    owner.reconciliation.store(2, Ordering::SeqCst);
    // This fixture supplies SafeToRetry only because its owner atomically
    // deduplicates the complete evidence under the same delivery identity.
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::RetryAuthorized
    ));
    owner.mode.store(0, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
    let late = owner.delayed.lock().unwrap().take().unwrap();
    assert!(owner.admission.lock().unwrap().as_ref() == Some(&late));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 2);
    assert_eq!(effect(&f, &ended).await.attempt, 2);
}

#[tokio::test]
async fn claimed_but_undispatched_crash_requires_reconciliation_after_lease_expiry() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Expired).await;
    let owner = Delivery::new(0);
    let key = effect_key(&ended);
    let EffectClaimOutcome::Claimed(claimed) = f
        .store
        .claim_effect(&key, ended.state().revision, "crashed-worker", 1000)
        .await
        .unwrap()
    else {
        panic!()
    };
    let lease = claimed.lease.unwrap();
    assert!(matches!(
        step(&f, &ended, &owner).await,
        Err(SetupError::Store(StoreError::Claimed))
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while f.store.now_ms().await.unwrap() <= lease.expires_at_ms {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::Pending
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(owner.reconciliations.load(Ordering::SeqCst), 1);
    assert!(matches!(
        f.store
            .acknowledge_effect(
                &lease,
                EffectOutcome::Uncertain {
                    evidence: RecordData {
                        schema: "test".into(),
                        version: 1,
                        body: serde_json::json!({})
                    }
                }
            )
            .await,
        Err(StoreError::Fenced)
    ));
}

#[tokio::test]
async fn definitive_rejection_requires_owner_confirmation_even_after_settlement() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(2);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.reconciliation.store(3, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::OwnerRejected
    ));
    assert_eq!(effect(&f, &ended).await.status, EffectStatus::Failed);
    assert!(matches!(
        step(&f, &ended, &owner).await.unwrap(),
        ContinuationStep::OwnerRejected
    ));
    owner.authorized.store(false, Ordering::SeqCst);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.authorized.store(true, Ordering::SeqCst);
    owner.reconciliation.store(1, Ordering::SeqCst);
    assert!(matches!(
        step(&f, &ended, &owner).await,
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store.get(ended.key()).await.unwrap().unwrap().revision,
        ended.state().revision
    );
}

#[tokio::test]
async fn lease_bounds_cover_both_owner_calls_and_settlement_with_checked_arithmetic() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(0);
    for (lease_ms, owner_timeout_ms, settlement_budget_ms) in [
        (120, 100, 1),
        (201, 100, 2),
        (200, 100, 0),
        (100, 0, 100),
        (86_400_001, 1, 1),
        (86_400_000, u64::MAX, 1),
        (86_400_000, 1, u64::MAX),
        (0, 1, 1),
    ] {
        assert!(matches!(
            f.bridge()
                .step_continuation(
                    &id(&ended.key().id),
                    &owner,
                    &ContinuationStepOptions {
                        worker: id("worker"),
                        lease_ms,
                        owner_timeout_ms,
                        settlement_budget_ms,
                    }
                )
                .await,
            Err(SetupError::Store(StoreError::Invalid(_)))
        ));
    }
    assert_eq!(owner.preflights.load(Ordering::SeqCst), 0);
    assert_eq!(owner.dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(effect(&f, &ended).await.attempt, 0);
    let settings = ContinuationStepOptions {
        worker: id("worker"),
        lease_ms: 1000,
        owner_timeout_ms: 400,
        settlement_budget_ms: 200,
    };
    assert!(matches!(
        f.bridge()
            .step_continuation(&id(&ended.key().id), &owner, &settings)
            .await
            .unwrap(),
        ContinuationStep::Acknowledged(_)
    ));
}

#[tokio::test]
async fn later_observation_authenticates_without_rewriting_the_first_receipt() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(0);
    owner.advance_observation.store(true, Ordering::SeqCst);
    assert!(matches!(step(&f, &ended, &owner).await.unwrap(), ContinuationStep::Acknowledged(_)));
    let first = f.store.get(ended.key()).await.unwrap().unwrap();
    assert!(matches!(step(&f, &ended, &owner).await.unwrap(), ContinuationStep::Acknowledged(_)));
    assert_eq!(first, f.store.get(ended.key()).await.unwrap().unwrap());
    owner.advance_observation.store(false, Ordering::SeqCst);
    owner.admission.lock().unwrap().as_mut().unwrap().observed_at_ms = 0;
    assert!(step(&f, &ended, &owner).await.is_err());
}

#[tokio::test]
async fn committed_source_reads_only_the_existing_terminal_record() {
    let f = Fixture::new();
    let fresh = f.admit().await;
    assert!(f.bridge().committed_continuation(&id(&fresh.key().id)).await.unwrap().is_none());
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let canonical = f.bridge().committed_continuation(&id(&ended.key().id)).await.unwrap().unwrap();
    assert_eq!(canonical.operation().as_str(), ended.key().id);
    assert_eq!(canonical.digest().unwrap(), continuation_obligation(&ended.record, &ended.request).unwrap().digest().unwrap());
    assert!(f.bridge().committed_continuation(&id("missing-operation")).await.unwrap().is_none());
}


#[tokio::test]
async fn recovery_refuses_failed_effect_with_non_rejection_observation() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let owner = Delivery::new(2);
    assert!(step(&f, &ended, &owner).await.is_err());
    owner.reconciliation.store(3, Ordering::SeqCst);
    assert!(matches!(step(&f, &ended, &owner).await.unwrap(), ContinuationStep::OwnerRejected));
    let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
    assert!(page.operations.is_empty());
    assert!(!page.had_unavailable_records);
    let saved = effect(&f, &ended).await;
    for kind in ["owner_pending", "safe_to_retry"] {
        let mut changed = saved.clone();
        let Some(EffectOutcome::Failed { evidence }) = &mut changed.outcome else { panic!("failed fixture"); };
        evidence.body["kind"]["kind"] = serde_json::Value::String(kind.into());
        corrupt_effect(&f, &changed);
        let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
        assert!(page.operations.is_empty());
        assert!(page.had_unavailable_records, "{kind}");
    }
}

#[tokio::test]
async fn recovery_requires_matching_completed_admission_with_local_observation_time_variance() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Failed).await;
    let owner = Delivery::new(0);
    assert!(matches!(step(&f, &ended, &owner).await.unwrap(), ContinuationStep::Acknowledged(_)));
    let saved = effect(&f, &ended).await;
    let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
    assert!(page.operations.is_empty());
    assert!(!page.had_unavailable_records);
    let mut later = saved.clone();
    let Some(EffectOutcome::Succeeded { receipt }) = &mut later.outcome else { panic!("succeeded fixture"); };
    let at = receipt.body["observed_at_ms"].as_u64().unwrap();
    receipt.body["observed_at_ms"] = serde_json::json!(at + 1);
    corrupt_effect(&f, &later);
    let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
    assert!(page.operations.is_empty());
    assert!(!page.had_unavailable_records);
    for field in ["runtime", "session", "receipt", "durable_input"] {
        let mut changed = saved.clone();
        let Some(EffectOutcome::Succeeded { receipt }) = &mut changed.outcome else { unreachable!(); };
        if matches!(field, "runtime" | "session") {
            receipt.body[field] = serde_json::Value::String("another-valid-id".into());
        } else {
            receipt.body[field]["id"] = serde_json::Value::String("another-valid-id".into());
        }
        corrupt_effect(&f, &changed);
        let page = f.bridge().continuation_recovery_page(None, 32).await.unwrap();
        assert!(page.operations.is_empty());
        assert!(page.had_unavailable_records, "{field}");
    }
}
