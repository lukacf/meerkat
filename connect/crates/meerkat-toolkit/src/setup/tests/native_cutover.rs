//! Cutover refusal and replay evidence against real, reopened SQLite.
use super::*;
use crate::work_admission::{
    SetupWorkAdmission, SetupWorkAdmissionError, SetupWorkAuthorizer,
    SetupWorkObservationAuthorizer,
};
use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService, WorkItemFilter, WorkNamespace};
use std::sync::{Arc, atomic::AtomicUsize};

mod old_fixtures;

// Observe all saved cells, including claims, admissions, raw JSON, events and
// effect outcomes. This does not claim byte equality of SQLite's physical file.
fn inventory(path: &std::path::Path) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    [
        "SELECT * FROM toolkit_metadata ORDER BY singleton",
        "SELECT * FROM toolkit_records ORDER BY realm,owner,id",
        "SELECT * FROM toolkit_admissions ORDER BY realm,owner,admission_key",
        "SELECT * FROM toolkit_events ORDER BY realm,owner,record_id,sequence",
        "SELECT * FROM toolkit_effects ORDER BY realm,owner,record_id,id",
    ]
    .into_iter()
    .map(|sql| {
        let mut statement = connection.prepare(sql).unwrap();
        let width = statement.column_count();
        statement
            .query_map([], |row| {
                (0..width)
                    .map(|column| row.get::<_, rusqlite::types::Value>(column))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    })
    .collect()
}

struct NoEffects(AtomicUsize);
impl NoEffects {
    fn entered(&self) -> ! {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("obsolete acquisition reached an effect or reference owner")
    }
}
impl ContextReferenceOwner for NoEffects {
    fn verify_access(
        &self,
        _: &ContextReference,
        _: &SetupRequestEvidence,
        _: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        self.entered()
    }
}
impl SetupWorkPreparer for NoEffects {
    fn prepare(
        &self,
        _: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        self.entered()
    }
}
#[async_trait]
impl SetupWorkAuthorizer for NoEffects {
    async fn admission_fence(
        &self,
        _: &SetupWorkAdmission,
        _: &meerkat_workgraph::WorkAdmission,
    ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError> {
        self.entered()
    }
}
#[async_trait]
impl SetupWorkObservationAuthorizer for NoEffects {
    async fn authorize_observation(
        &self,
        _: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.entered()
    }
    fn with_current_disclosure(
        &self,
        _: &SetupWorkAdmission,
        _: &meerkat_workgraph::WorkAdmission,
        _: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.entered()
    }
}
#[async_trait]
impl SetupDeadlineOwner for NoEffects {
    async fn authorize_expiry(&self, _: &SetupSnapshot) -> Result<(), SetupError> {
        self.entered()
    }
}
#[async_trait]
impl SetupContinuationOwner for NoEffects {
    async fn verify_admission(
        &self,
        _: &ContinuationObligation,
        _: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.entered()
    }
}
#[async_trait]
impl SetupContinuationDelivery for NoEffects {
    async fn authorize_delivery(
        &self,
        _: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        self.entered()
    }
    async fn admit(
        &self,
        _: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.entered()
    }
    async fn reconcile(
        &self,
        _: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        self.entered()
    }
}

async fn seed_old(f: &Fixture, terminal: bool, version: u32) -> StoreKey {
    let mut admission: AdmissionRequest =
        serde_json::from_str(old_fixtures::PENDING_ADMISSION).unwrap();
    admission.data.version = version;
    let key = admission.key.clone();
    let initial = f.store.admit(admission).await.unwrap();
    let prepared: AdmissionRequest =
        serde_json::from_str(old_fixtures::PREPARED_ADMISSION).unwrap();
    f.store.admit(prepared).await.unwrap();
    if terminal {
        let mut data: RecordData = serde_json::from_str(old_fixtures::TERMINAL_RECORD).unwrap();
        data.version = version;
        let effect: EffectIntent = serde_json::from_str(old_fixtures::CONTINUATION_EFFECT).unwrap();
        let changed = f
            .store
            .compare_and_swap(Transition {
                key: key.clone(),
                expected_revision: initial.record().revision,
                expected_digest: Some(initial.record().content_digest.clone()),
                claim: None,
                data,
                events: vec![EventWrite {
                    kind: effect.kind.clone(),
                    data: effect.request.clone(),
                }],
                effects: vec![effect.clone()],
            })
            .await
            .unwrap();
        let effect_key = EffectKey {
            record: key.clone(),
            id: effect.id,
        };
        let EffectClaimOutcome::Claimed(claimed) = f
            .store
            .claim_effect(&effect_key, changed.revision, "old-executor", 10000)
            .await
            .unwrap()
        else {
            panic!("first old continuation claim")
        };
        let uncertain = f
            .store
            .acknowledge_effect(
                claimed.lease.as_ref().unwrap(),
                EffectOutcome::Uncertain {
                    evidence: serde_json::from_str(old_fixtures::UNCERTAIN_OBSERVATION).unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(uncertain.status, EffectStatus::Uncertain);
        assert_eq!(uncertain.attempt, 1);
    }
    key
}

fn assert_refusal<T>(result: Result<T, SetupError>, version: u32) {
    if version == 4 {
        // The old claims-only body cannot become native evidence by relabelling its row.
        assert!(matches!(result, Err(SetupError::Encoding(_))));
    } else {
        assert!(matches!(result, Err(SetupError::UnsupportedRecordVersion)));
    }
}

#[tokio::test]
async fn frozen_old_records_refuse_all_public_effect_routes_without_mutation() {
    for (terminal, version) in [(false, 2), (true, 2), (true, 4), (false, 3)] {
        let f = Fixture::new();
        let key = seed_old(&f, terminal, version).await;
        let path = f.directory.path().join("toolkit.sqlite");
        let reopened = SqliteStore::open(&path).unwrap();
        let bridge = f.for_store(&reopened);
        let operation = id(&key.id);
        let owner = NoEffects(0.into());
        let context = f.context(200);
        let access = || SetupAccess {
            evidence: &f.authority,
            context: &context,
            references: &owner,
        };
        let service = WorkGraphService::with_scope(
            Arc::new(SqliteWorkGraphStore::open(f.directory.path().join("work.sqlite")).unwrap()),
            "tenant-a",
            WorkNamespace::new("setup").unwrap(),
        );
        let host = SetupWorkHost {
            backend: SetupWorkBackend::new(
                service.clone(),
                "fixture-sqlite".into(),
                id("fixture-workgraph"),
                &[meerkat_core::DurabilityDeclaration::durable(
                    "workgraph",
                    meerkat_core::DurabilityResolution::Persistent,
                )],
            )
            .unwrap(),
            dispatch: &owner,
            observation: &owner,
        };
        let before = inventory(&path);
        assert_refusal(
            bridge
                .load(&operation, &f.authority, &context, &owner)
                .await,
            version,
        );
        assert_refusal(
            bridge.prepare_work(&operation, access(), &owner).await,
            version,
        );
        assert_refusal(
            bridge
                .step_work_admission(
                    &operation,
                    &host,
                    Some(access()),
                    &WorkAdmissionStepOptions {
                        worker: id("new-worker"),
                        lease_ms: 2000,
                        owner_timeout_ms: 100,
                        settlement_budget_ms: 1000,
                    },
                )
                .await,
            version,
        );
        assert_refusal(bridge.expire(&operation, &owner).await, version);
        assert_refusal(
            bridge
                .step_continuation(
                    &operation,
                    &owner,
                    &ContinuationStepOptions {
                        worker: id("new-worker"),
                        lease_ms: 2000,
                        owner_timeout_ms: 100,
                        settlement_budget_ms: 1000,
                    },
                )
                .await,
            version,
        );
        assert_refusal(
            bridge
                .acknowledge_continuation(&operation, &continuation_receipt(), &owner)
                .await,
            version,
        );
        assert_eq!(owner.0.load(Ordering::SeqCst), 0);
        assert_eq!(inventory(&path), before);
        assert!(
            service
                .store()
                .list_items(WorkItemFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn reopened_correlation_conflicts_with_valid_changed_payloads() {
    let f = Fixture::new();
    let original = f.admit().await;
    let path = f.directory.path().join("toolkit.sqlite");
    let before = inventory(&path);
    let reopened = SqliteStore::open(&path).unwrap();
    let bridge = f.for_store(&reopened);
    let replay = bridge
        .admit(&f.request, &f.authority, &f.context(200), &f.references)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.snapshot.key(), original.key());
    for change in 0..8 {
        let mut constraints = f.authority.constraints().clone();
        let mut association = f.authority.association().candidate().clone();
        let mut raw = f.request.request().clone();
        let mut host = f.request.host().clone();
        let destination = f.destination.clone();
        let mut policy = f.policy.clone();
        match change {
            0 => {
                policy = rev("policy-2");
                constraints.policy_revision = policy.clone();
            }
            1 => association.ingress_actor.kind = PrincipalKind::PersonalAgent,
            2 => association.root_event = native_fixture::event("different-origin"),
            3 => association
                .source_observations
                .push(native_fixture::event("different-source")),
            4 => {
                association.original_authentication =
                    native_fixture::event("different-authentication");
            }
            5 => host.continuation.delivery_key = id("different-delivery"),
            6 => raw.request.push_str(" Additional original-user detail."),
            7 => host.parent_operation = Some(id("different-parent")),
            _ => unreachable!(),
        }
        let authority = SetupRequestEvidence::new(
            InputAuthorityAssociation::new(association).unwrap(),
            constraints,
        )
        .unwrap();
        let authorization = native_fixture::current(authority.association());
        let context = SetupUseContext {
            tenant: &f.tenant,
            deployment: &f.deployment,
            realm: &authority.association().candidate().ingress_namespace.realm,
            target: &destination,
            authorization: &authorization,
            policy_revision: &policy,
            now_ms: 200,
        };
        let candidate = validate_request(raw, host, &authority, &context, &f.references).unwrap();
        assert_eq!(
            candidate.correlation_digest(),
            f.request.correlation_digest()
        );
        assert_eq!(
            operation_id(&candidate).unwrap().as_str(),
            original.key().id
        );
        assert_ne!(candidate.input_digest(), f.request.input_digest());
        assert!(matches!(
            bridge
                .admit(&candidate, &authority, &context, &f.references)
                .await,
            Err(SetupError::Store(StoreError::AdmissionConflict))
        ));
        assert_eq!(inventory(&path), before);
    }
    let records = reopened
        .list_records(
            &StoreScope {
                realm: f.tenant.to_string(),
                owner: original.key().owner.clone(),
            },
            None,
            100,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let effects = reopened
        .list_effects(original.key(), None, 100)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].intent.kind, "work_admission_requested");
}

struct ExactNativeContinuation {
    requester: PrincipalRef,
    origin: meerkat_authorization_contracts::evidence::HistoricalEvidenceRef,
    original_work: meerkat_authorization_contracts::work_association::OriginalWorkRef,
    calls: AtomicUsize,
}
#[async_trait]
impl SetupContinuationOwner for ExactNativeContinuation {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(obligation.requester(), &self.requester);
        assert!(obligation.origin() == &self.origin);
        assert_eq!(obligation.original_work(), &self.original_work);
        Ok(Some(ContinuationAdmissionEvidence {
            receipt: receipt.clone(),
            obligation_digest: obligation.digest()?,
            delivery_identity: obligation.delivery_identity()?,
            runtime: id("runtime-a"),
            session: id("session-a"),
            durable_input: reference("native-continuation-input"),
            observed_at_ms: obligation.terminal_at_ms() + 1,
        }))
    }
}

#[tokio::test]
async fn native_identity_width_and_kind_survive_reopen_and_continuation() {
    let mut f = Fixture::new();
    let mut association = f.authority.association().candidate().clone();
    association.requester = native_fixture::principal(
        PrincipalKind::ServiceAccount,
        &format!("service principal:{}", "x".repeat(210)),
    );
    association.root_event = native_fixture::event(&format!("origin:{}", "y".repeat(210)));
    f.authority = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(association).unwrap(),
        f.authority.constraints().clone(),
    )
    .unwrap();
    f.authorization = native_fixture::current(f.authority.association());
    f.request = validate_request(
        f.request.request().clone(),
        f.request.host().clone(),
        &f.authority,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    let initial = f.admit().await;
    let operation = id(&initial.key().id);
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let bridge = f.for_store(&reopened);
    let recovered = bridge
        .load(&operation, &f.authority, &f.context(200), &f.references)
        .await
        .unwrap();
    assert_eq!(
        recovered
            .request()
            .evidence()
            .association()
            .candidate()
            .requester,
        f.authority.association().candidate().requester
    );
    assert!(
        recovered
            .request()
            .evidence()
            .association()
            .candidate()
            .root_event
            == f.authority.association().candidate().root_event
    );
    let lease = reopened
        .claim_record(
            recovered.key(),
            recovered.state().revision,
            "cancel-owner",
            10000,
        )
        .await
        .unwrap();
    bridge
        .resolve(
            &recovered,
            Resolution::Cancelled(reference("cancelled")),
            Some(&lease),
            f.access(&f.context(220)),
        )
        .await
        .unwrap();
    let owner = ExactNativeContinuation {
        requester: f.authority.association().candidate().requester.clone(),
        origin: f.authority.association().candidate().root_event.clone(),
        original_work: f.authority.original_work().clone(),
        calls: 0.into(),
    };
    let cold = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let receipt = f
        .for_store(&cold)
        .acknowledge_continuation(&operation, &continuation_receipt(), &owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.operation, operation);
    assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
}
