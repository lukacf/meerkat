use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn request(id: &str) -> AdmissionRequest {
    AdmissionRequest {
        key: StoreKey {
            realm: "realm".into(),
            owner: "owner".into(),
            id: id.into(),
        },
        admission_key: id.into(),
        data: RecordData {
            schema: "test.clock".into(),
            version: 1,
            body: serde_json::json!({}),
        },
        events: vec![],
        effects: vec![],
    }
}

#[tokio::test]
async fn expiry_after_final_scan_refuses_and_rolls_back_actual_admission() {
    for expiry_kind in ["deadline", "record", "dispatch"] {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let mut owner_request = request("policy");
        owner_request.effects.push(EffectIntent {
            id: "effect".into(),
            kind: "test.effect".into(),
            request: owner_request.data.clone(),
            idempotency_key: "effect".into(),
            replay_safety: ReplaySafety::ReconcileBeforeRetry,
        });
        let owner = store.admit(owner_request).await.unwrap().record().clone();
        let base = store.now_ms().await.unwrap();
        let condition = RecordCondition {
            key: owner.key.clone(),
            revision: owner.revision,
            content_digest: owner.content_digest,
            claim: None,
            dispatch_claim: None,
        };
        let mut candidate = ConditionalAdmission {
            admission: request("child"),
            conditions: vec![condition],
            before_ms: base + 100_000,
        };
        candidate.admission.events.push(EventWrite {
            kind: "child-published".into(),
            data: candidate.admission.data.clone(),
        });
        candidate.admission.effects.push(EffectIntent {
            id: "child-effect".into(),
            kind: "test.child".into(),
            request: candidate.admission.data.clone(),
            idempotency_key: "child-effect".into(),
            replay_safety: ReplaySafety::ReconcileBeforeRetry,
        });
        let expiry = match expiry_kind {
            "deadline" => {
                candidate.before_ms = base + 10_000;
                candidate.before_ms
            }
            "record" => {
                let lease = store
                    .claim_record(&owner.key, owner.revision, "worker", 10_000)
                    .await
                    .unwrap();
                let expiry = lease.expires_at_ms;
                candidate.conditions[0].claim = Some(lease);
                expiry
            }
            "dispatch" => {
                let key = EffectKey {
                    record: owner.key.clone(),
                    id: "effect".into(),
                };
                let EffectClaimOutcome::Claimed(effect) = store
                    .claim_effect(&key, owner.revision, "worker", 10_000)
                    .await
                    .unwrap()
                else {
                    panic!("fresh claim")
                };
                let lease = effect.lease.unwrap();
                let expiry = lease.expires_at_ms;
                candidate.conditions[0].dispatch_claim = Some(lease);
                expiry
            }
            _ => unreachable!(),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let clock_calls = calls.clone();
        // Only this private test connection's SQLite time is scripted. Run the
        // real public operation, real condition scans and actual transaction.
        // First checks and the insert see live time; the final decision sees
        // a time safely past expiry despite floating-point SQL conversion.
        // Reusing the pre-scan sample incorrectly commits the child.
        store
            .with(move |connection, _| {
                connection.create_scalar_function(
                    "unixepoch",
                    1,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    move |_| {
                        let call = clock_calls.fetch_add(1, Ordering::SeqCst);
                        let time = if call < 3 { base } else { expiry + 1_000 };
                        Ok(time as f64 / 1000.0)
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let error = store.admit_conditionally(candidate).await.unwrap_err();
        if expiry_kind == "deadline" {
            assert!(matches!(error, StoreError::AdmissionExpired));
        } else {
            assert!(matches!(error, StoreError::Fenced));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let child = request("child").key;
        assert!(store.get(&child).await.unwrap().is_none());
        assert!(store.read_events(&child, 0, 100).await.unwrap().is_empty());
        assert!(
            store
                .list_effects(&child, None, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
