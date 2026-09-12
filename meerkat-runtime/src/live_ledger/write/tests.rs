use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::live_resources::LIVE_LEDGER_MAX_CHARGE;
use crate::store::live_read::{
    LiveCompositeReadRequest, LiveLedgerWriteProfile, RuntimeLiveLedgerOps, read_live_composite,
};
use crate::store::{
    InMemoryRuntimeStore, RuntimeStore, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
    SerializedSessionSnapshot,
};
use meerkat_core::{Session, SessionStore};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug)]
enum Backend {
    Memory,
    #[cfg(feature = "sqlite-store")]
    WholeBlob,
    #[cfg(feature = "sqlite-store")]
    HeadCanonical,
}

fn backends() -> Vec<Backend> {
    vec![
        Backend::Memory,
        #[cfg(feature = "sqlite-store")]
        Backend::WholeBlob,
        #[cfg(feature = "sqlite-store")]
        Backend::HeadCanonical,
    ]
}

struct Fixture {
    store: Arc<dyn RuntimeStore>,
    session: Session,
    _directory: tempfile::TempDir,
    #[cfg(feature = "sqlite-store")]
    path: std::path::PathBuf,
}

impl Fixture {
    async fn new(backend: Backend) -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let session = Session::new();
        #[cfg(feature = "sqlite-store")]
        let path = directory.path().join("runtime.sqlite3");
        let store: Arc<dyn RuntimeStore> = match backend {
            Backend::Memory => {
                let store = InMemoryRuntimeStore::new();
                save_actor(&store, &session).await?;
                Arc::new(store)
            }
            #[cfg(feature = "sqlite-store")]
            Backend::WholeBlob | Backend::HeadCanonical => {
                if matches!(backend, Backend::HeadCanonical) {
                    meerkat_store::SqliteSessionStore::open(&path)?
                        .save(&session)
                        .await?;
                }
                let initial = crate::store::SqliteRuntimeStore::new_whole_blob(&path)?;
                save_actor(&initial, &session).await?;
                if matches!(backend, Backend::HeadCanonical) {
                    drop(initial);
                    Arc::new(crate::store::SqliteRuntimeStore::new_head_canonical(&path)?)
                } else {
                    Arc::new(initial)
                }
            }
        };
        Ok(Self {
            store,
            session,
            _directory: directory,
            #[cfg(feature = "sqlite-store")]
            path,
        })
    }

    fn ops(&self) -> TestResult<&dyn RuntimeLiveLedgerOps> {
        self.store
            .live_ledger_ops()
            .ok_or_else(|| "missing Live capability".into())
    }

    async fn actor(&self) -> TestResult<RuntimeSessionAuthority> {
        self.store
            .load_session_boundary_authority(&LogicalRuntimeId::for_session(self.session.id()))
            .await?
            .ok_or_else(|| "actor authority missing".into())
    }
}

async fn save_actor(store: &dyn RuntimeStore, session: &Session) -> TestResult {
    store
        .commit_session_snapshot(
            &LogicalRuntimeId::for_session(session.id()),
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(session)?),
            },
        )
        .await?;
    Ok(())
}

#[derive(Clone)]
struct Fence(RuntimeStoreWriteFenceOutcome);

impl RuntimeStoreWriteFence for Fence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        if self.0 == RuntimeStoreWriteFenceOutcome::Applied {
            operation()?;
        }
        Ok(self.0.clone())
    }
}

fn current_fence() -> Arc<dyn RuntimeStoreWriteFence> {
    Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Applied))
}

fn observation(sequence: u64, text: &str) -> TestResult<LiveLedgerRecord> {
    use meerkat_contracts::wire::live_observation::{
        LiveObservationRecord, LiveObservationWireCodecV1,
    };
    use meerkat_core::live_observation::{
        LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(sequence)?,
        channel_id: meerkat_core::live_execution::LiveChannelId::new("voice"),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(0.0, 1.0)?,
            text,
        ),
    })?;
    Ok(LiveLedgerRecord::Observation(
        super::super::transcript::StoredLiveObservation::from_fit(&fit),
    ))
}

// Only this cfg(test) child can construct a synthetic prepared transition.
// Production construction remains reserved for the generated Live owner.
fn prepared(
    session_id: &SessionId,
    before: Option<&LiveLedgerStoredHead>,
    records: Vec<LiveLedgerRecord>,
) -> TestResult<PreparedLiveLedgerCommit> {
    let generation = before.map_or(1, |head| head.reference.generation);
    let mut reference = before.map_or_else(
        || LiveHeadReference {
            format: LiveLedgerFormatV1::V1,
            session_id: session_id.clone(),
            generation,
            revision: 0,
            event_count: 0,
            prefix_digest: LiveLedgerPrefixDigest::empty(session_id, generation),
        },
        |head| head.reference.clone(),
    );
    reference.revision += 1;
    let mut payload = before.map_or_else(
        || LiveLedgerPayloadState {
            used: LiveResourceCharge {
                records: 0,
                encoded_bytes: LIVE_HEAD_STORAGE_ALLOWANCE_BYTES,
            },
            reserved: LiveResourceCharge::default(),
            ingress_generation: 1,
            transcript_snapshot: Arc::new(Vec::new()),
            request_snapshot: Arc::new(Vec::new()),
        },
        |head| head.payload.clone(),
    );
    for record in &records {
        let bytes = record.encode()?;
        reference.event_count += 1;
        reference.prefix_digest = reference.prefix_digest.appended(record.sequence(), &bytes);
        payload.used = payload
            .used
            .checked_add(LiveResourceCharge::for_event_record(&bytes)?)?;
    }
    Ok(PreparedLiveLedgerCommit {
        expected: before.map(|head| head.reference.clone()),
        expected_actor: None,
        successor: LiveLedgerStoredHead { reference, payload },
        records,
        sources: Vec::new(),
        quota: LIVE_LEDGER_MAX_CHARGE,
    })
}

fn copy_prepared(value: &PreparedLiveLedgerCommit) -> PreparedLiveLedgerCommit {
    PreparedLiveLedgerCommit {
        expected: value.expected.clone(),
        expected_actor: value.expected_actor.clone(),
        successor: value.successor.clone(),
        records: value.records.clone(),
        sources: value
            .sources
            .iter()
            .map(|source| super::super::source::PreparedLiveSourceMutation {
                expected: source.expected,
                replacement: source.replacement.clone(),
            })
            .collect(),
        quota: value.quota,
    }
}

async fn reserved_source(
    fixture: &Fixture,
    delegation: &str,
) -> TestResult<super::super::source::LiveSourceRow> {
    use crate::live_source::{
        LiveSourceContextReference, LiveSourceEntryRecord, LiveSourceFingerprint,
    };
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_execution::evidence::LiveObservationInterval;
    use meerkat_core::live_execution::request::{
        LiveProviderReference, LiveSourceIdentity, LiveSourceKey,
    };
    let source = LiveSourceKey::new(
        fixture.session.id().clone(),
        LiveChannelId::new("voice"),
        LiveSourceIdentity::ClientDelegation {
            delegation: LiveProviderReference::new(delegation)?,
        },
    )?;
    let read = read_live_composite(
        fixture.ops()?,
        LiveCompositeReadRequest::new(
            fixture.session.id().clone(),
            Some(LiveChannelId::new("voice")),
            0,
            64,
        )?,
    )
    .await?
    .ok_or("composite")?;
    let interval =
        LiveObservationInterval::new(0, read.authority().live_head().ok_or("head")?.event_count)?;
    let context = LiveSourceContextReference::from_composite(&read, &source, interval)?;
    let record = serde_json::from_value(serde_json::json!({
        "source": source, "request_id": uuid::Uuid::new_v4(),
        "fingerprint": LiveSourceFingerprint::client_delegation(2.5)?,
        "context": context,
        "frozen_request": {"kind":"application_snapshot","observations":interval,"request":" original request "},
        "grant": {"id":uuid::Uuid::new_v4(),"issuer_realm":"owner","generation":1},
        "cancellation":null,"disposition":{"kind":"reserved"}
    }))?;
    Ok(super::super::source::LiveSourceRow::encode(
        &LiveSourceEntryRecord::Reservation {
            record: Box::new(record),
        },
    )?)
}

fn add_source(
    change: &mut PreparedLiveLedgerCommit,
    current: Option<&super::super::source::LiveSourceRow>,
    replacement: super::super::source::LiveSourceRow,
) -> TestResult {
    if let Some(current) = current {
        change.successor.payload.used = change
            .successor
            .payload
            .used
            .checked_sub(current.charge()?)?;
    }
    change.successor.payload.used = change
        .successor
        .payload
        .used
        .checked_add(replacement.charge()?)?;
    change
        .sources
        .push(super::super::source::PreparedLiveSourceMutation {
            expected: current.map(super::super::source::LiveSourceRow::digest),
            replacement,
        });
    Ok(())
}

#[tokio::test]
async fn source_cas_commits_with_head_events_and_preserves_immutable_reservation() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let start = prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?;
        fixture
            .ops()?
            .commit_live_ledger(start, current_fence())
            .await?;
        let source = reserved_source(&fixture, "\0opaque-source-key").await?;
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut reserve = prepared(
            fixture.session.id(),
            Some(&before),
            vec![observation(2, "concurrent append")?],
        )?;
        reserve.expected_actor = Some(fixture.actor().await?);
        add_source(&mut reserve, None, source.clone())?;
        let replay = copy_prepared(&reserve);
        fixture
            .ops()?
            .commit_live_ledger(reserve, current_fence())
            .await?;
        let stored = fixture
            .ops()?
            .lookup_live_source(source.source())
            .await?
            .ok_or("source")?;
        assert_eq!(stored.bytes(), source.bytes());
        assert_eq!(stored.digest(), source.digest());
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let current_head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut duplicate = prepared(fixture.session.id(), Some(&current_head), vec![])?;
        add_source(&mut duplicate, None, source.clone())?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(duplicate, current_fence())
                .await?,
            LiveLedgerCommitOutcome::SourceConflict { .. }
        ));
        for field in [
            "request_id",
            "fingerprint",
            "frozen_request",
            "grant",
            "context",
        ] {
            let mut value = serde_json::to_value(source.record()?)?;
            let record = &mut value["record"];
            match field {
                "request_id" => record[field] = serde_json::json!(uuid::Uuid::new_v4()),
                "fingerprint" => record[field] = serde_json::json!(vec![9; 32]),
                "frozen_request" => record[field]["request"] = serde_json::json!("changed request"),
                "grant" => record[field]["generation"] = serde_json::json!(2),
                "context" => record[field]["actor"]["revision"] = serde_json::json!(2),
                _ => unreachable!(),
            }
            let replacement =
                super::super::source::LiveSourceRow::encode(&serde_json::from_value(value)?)?;
            let mut mutation = prepared(fixture.session.id(), Some(&current_head), vec![])?;
            add_source(&mut mutation, Some(&source), replacement)?;
            assert!(
                fixture
                    .ops()?
                    .commit_live_ledger(mutation, current_fence())
                    .await
                    .is_err(),
                "{backend:?} {field}"
            );
            assert_eq!(
                fixture
                    .ops()?
                    .lookup_live_source(source.source())
                    .await?
                    .ok_or("source")?
                    .bytes(),
                source.bytes()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_replacement_charges_deltas_and_replay_binds_all_source_expectations() -> TestResult
{
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, "replace").await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut initial = prepared(fixture.session.id(), Some(&before), vec![])?;
        initial.expected_actor = Some(fixture.actor().await?);
        add_source(&mut initial, None, source.clone())?;
        fixture
            .ops()?
            .commit_live_ledger(initial, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut value = serde_json::to_value(source.record()?)?;
        value["record"]["cancellation"] = serde_json::json!("operator_requested");
        value["record"]["disposition"] =
            serde_json::json!({"kind":"cancelled_without_run","reason":"operator_requested"});
        let replacement =
            super::super::source::LiveSourceRow::encode(&serde_json::from_value(value)?)?;
        let mut update = prepared(fixture.session.id(), Some(&before), vec![])?;
        add_source(&mut update, Some(&source), replacement.clone())?;
        let replay = copy_prepared(&update);
        fixture
            .ops()?
            .commit_live_ledger(update, current_fence())
            .await?;
        let after = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.payload.used.records, before.payload.used.records);
        assert_eq!(
            after.payload.used.encoded_bytes,
            before.payload.used.encoded_bytes - source.charge()?.encoded_bytes
                + replacement.charge()?.encoded_bytes
        );
        let mut changed_expectation = copy_prepared(&replay);
        changed_expectation.sources[0].expected = None;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(changed_expectation, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        let mut omitted = copy_prepared(&replay);
        omitted.sources.clear();
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(omitted, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn initial_source_reservation_requires_its_composite_actor_and_head_fence() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, "fenced").await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut missing_actor = prepared(fixture.session.id(), Some(&head), vec![])?;
        add_source(&mut missing_actor, None, source.clone())?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(missing_actor, current_fence())
                .await
                .is_err()
        );
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(
                    fixture.session.id(),
                    Some(&head),
                    vec![observation(2, "new frontier")?],
                )?,
                current_fence(),
            )
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut wrong_frontier = prepared(fixture.session.id(), Some(&head), vec![])?;
        wrong_frontier.expected_actor = Some(fixture.actor().await?);
        add_source(&mut wrong_frontier, None, source.clone())?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(wrong_frontier, current_fence())
                .await
                .is_err()
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_late_source_failure_rolls_back_all_sources_head_events_and_receipt() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let first = reserved_source(&fixture, "first").await?;
        let second = reserved_source(&fixture, "second").await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut change = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "atomic")?],
        )?;
        change.expected_actor = Some(fixture.actor().await?);
        add_source(&mut change, None, first.clone())?;
        add_source(&mut change, None, second.clone())?;
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        let receipt: Vec<u8> =
            conn.query_row("SELECT commit_digest FROM runtime_live_heads", [], |row| {
                row.get(0)
            })?;
        conn.execute_batch(
            "CREATE TRIGGER reject_second_source BEFORE INSERT ON runtime_live_sources
             WHEN (SELECT count(*) FROM runtime_live_sources)=1
             BEGIN SELECT RAISE(ABORT, 'injected late source failure'); END;",
        )?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(first.source())
                .await?
                .is_none()
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(second.source())
                .await?
                .is_none()
        );
        assert_eq!(
            conn.query_row("SELECT commit_digest FROM runtime_live_heads", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })?,
            receipt
        );
        conn.execute_batch("DROP TRIGGER reject_second_source")?;
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(first.source())
                .await?
                .ok_or("source")?
                .bytes(),
            first.bytes()
        );
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(second.source())
                .await?
                .ok_or("source")?
                .bytes(),
            second.bytes()
        );
    }
    Ok(())
}

#[tokio::test]
async fn source_key_bytes_and_external_fence_are_part_of_atomic_publication() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                current_fence(),
            )
            .await?;
        let source = reserved_source(&fixture, &"\0".repeat(64)).await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut change = prepared(fixture.session.id(), Some(&head), vec![])?;
        change.expected_actor = Some(fixture.actor().await?);
        add_source(&mut change, None, source.clone())?;
        let mut missing_key_charge = copy_prepared(&change);
        missing_key_charge.successor.payload.used.encoded_bytes -=
            super::super::source::encoded_source_identity(source.source())?.len() as u64;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(missing_key_charge, current_fence())
                .await
                .is_err()
        );
        let fence = Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Conflict {
            reason: "revoked epoch".into(),
        }));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), fence)
                .await,
            Err(RuntimeStoreError::WriteFenceConflict { .. })
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        assert!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .is_none()
        );
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn source_rows_survive_reopen_and_corruption_cannot_become_an_absent_row() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for corrupt in [
            "UPDATE runtime_live_sources SET record=zeroblob(1048576)",
            "UPDATE runtime_live_sources SET record_digest=zeroblob(1048576)",
            "UPDATE runtime_live_sources SET record_digest=zeroblob(32)",
            "PRAGMA foreign_keys=OFF; DELETE FROM runtime_live_heads; DELETE FROM runtime_live_events;",
        ] {
            let fixture = Fixture::new(backend).await?;
            fixture
                .ops()?
                .commit_live_ledger(
                    prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
                    current_fence(),
                )
                .await?;
            let source = reserved_source(&fixture, "reopen").await?;
            let head = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            let mut change = prepared(fixture.session.id(), Some(&head), vec![])?;
            change.expected_actor = Some(fixture.actor().await?);
            add_source(&mut change, None, source.clone())?;
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?;
            let reopened = match backend {
                Backend::WholeBlob => {
                    crate::store::SqliteRuntimeStore::new_whole_blob(&fixture.path)?
                }
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&fixture.path)?
                }
                Backend::Memory => unreachable!(),
            };
            let loaded = reopened
                .lookup_live_source(source.source())
                .await?
                .ok_or("source")?;
            assert_eq!(loaded.bytes(), source.bytes());
            assert_eq!(loaded.digest(), source.digest());
            let conn =
                meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
            conn.pragma_update(None, "ignore_check_constraints", "ON")?;
            conn.execute_batch(corrupt)?;
            assert!(
                matches!(
                    reopened.lookup_live_source(source.source()).await,
                    Err(RuntimeStoreError::ReadFailed(_))
                ),
                "{backend:?} {corrupt}"
            );
            if corrupt.contains("DELETE") {
                assert!(
                    read_live_composite(
                        &reopened,
                        LiveCompositeReadRequest::new(fixture.session.id().clone(), None, 0, 64,)?
                    )
                    .await
                    .is_err()
                );
                assert!(
                    reopened
                        .commit_live_ledger(
                            prepared(fixture.session.id(), None, vec![])?,
                            current_fence()
                        )
                        .await
                        .is_err()
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn source_channel_storage_bounds_match_across_backends_before_publication() -> TestResult {
    use crate::live_source::LiveSourceEntryRecord;
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_execution::request::{
        LiveProviderReference, LiveRequestCancelIntent, LiveRequestCancellationReason,
        LiveSourceIdentity, LiveSourceKey,
    };
    let mut violations = Vec::new();
    for backend in backends() {
        for (channel, valid) in [
            ("x".repeat(128), true),
            ("x".repeat(129), false),
            ("\u{e9}".repeat(64), true),
            (format!("{}x", "\u{e9}".repeat(64)), false),
            ("\u{1f680}".repeat(32), true),
            (format!("{}x", "\u{1f680}".repeat(32)), false),
        ] {
            let fixture = Fixture::new(backend).await?;
            let source = LiveSourceKey::new(
                fixture.session.id().clone(),
                LiveChannelId::new(&channel),
                LiveSourceIdentity::ClientDelegation {
                    delegation: LiveProviderReference::new("d")?,
                },
            )?;
            let record = LiveSourceEntryRecord::CancellationOnly {
                intent: LiveRequestCancelIntent {
                    source: source.clone(),
                    reason: LiveRequestCancellationReason::OperatorRequested,
                },
            };
            match super::super::source::LiveSourceRow::encode(&record) {
                Ok(row) => {
                    let mut change = prepared(fixture.session.id(), None, vec![])?;
                    add_source(&mut change, None, row)?;
                    let result = fixture
                        .ops()?
                        .commit_live_ledger(change, current_fence())
                        .await;
                    if valid {
                        assert!(matches!(result?, LiveLedgerCommitOutcome::Committed { .. }));
                        assert!(fixture.ops()?.lookup_live_source(&source).await?.is_some());
                    } else {
                        violations.push(format!(
                            "{backend:?}: channel_bytes={} encode accepted; publication={result:?}",
                            channel.len()
                        ));
                    }
                }
                Err(error) => {
                    assert!(!valid, "{backend:?}: valid boundary refused: {error}");
                    assert!(
                        fixture
                            .ops()?
                            .load_live_head(fixture.session.id())
                            .await?
                            .is_none()
                    );
                    assert!(fixture.ops()?.lookup_live_source(&source).await.is_err());
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "invalid source keys reached publication: {violations:?}"
    );
    Ok(())
}

#[tokio::test]
async fn captured_history_prefix_survives_later_event_and_metadata_commits() -> TestResult {
    use crate::store::live_history::LiveHistoryReadRequest;
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "one")?, observation(2, "two")?],
        )?;
        let event_head = first.successor.reference.clone();
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let metadata = prepared(fixture.session.id(), Some(&before), vec![])?;
        let metadata_head = metadata.successor.reference.clone();
        fixture
            .ops()?
            .commit_live_ledger(metadata, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        fixture
            .ops()?
            .commit_live_ledger(
                prepared(
                    fixture.session.id(),
                    Some(&before),
                    vec![observation(3, "not in captured prefix")?],
                )?,
                current_fence(),
            )
            .await?;
        for captured in [event_head, metadata_head] {
            let mut after = 0;
            let mut reconstructed = Vec::new();
            loop {
                let request = LiveHistoryReadRequest::new(captured.clone(), None, after, 1)?;
                let window = fixture.ops()?.read_live_history(&request).await?;
                assert_eq!(window.head(), &captured);
                for record in window.records() {
                    reconstructed.push(record.encode()?);
                }
                if !window.has_more() {
                    break;
                }
                let next = window
                    .records()
                    .last()
                    .ok_or("non-progressing page")?
                    .sequence()
                    .get();
                assert!(next > after);
                after = next;
            }
            assert_eq!(
                reconstructed,
                vec![
                    observation(1, "one")?.encode()?,
                    observation(2, "two")?.encode()?
                ]
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_middle_of_atomic_batch_is_not_a_historical_head_even_with_its_real_prefix_hash()
-> TestResult {
    use crate::store::live_history::{LiveHistoryReadError, LiveHistoryReadRequest};
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(
            fixture.session.id(),
            None,
            vec![
                observation(1, "one")?,
                observation(2, "two")?,
                observation(3, "three")?,
            ],
        )?;
        let mut invented = first.successor.reference.clone();
        invented.event_count = 2;
        invented.prefix_digest = first
            .prefix_witnesses(&first.encoded_records()?)
            .nth(1)
            .ok_or("prefix witness")?
            .prefix;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let request = LiveHistoryReadRequest::new(invented, None, 0, 64)?;
        assert!(matches!(
            fixture.ops()?.read_live_history(&request).await,
            Err(LiveHistoryReadError::InvalidSnapshot)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn head_and_events_commit_together_without_changing_actor_authority() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let actor = fixture.actor().await?;
        let ops = fixture.ops()?;
        assert_eq!(
            ops.ledger_write_profile(),
            LiveLedgerWriteProfile::AtomicHeadEventsSources
        );
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "one")?, observation(2, "two")?],
        )?;
        let expected = change.successor.clone();
        assert_eq!(
            ops.commit_live_ledger(change, current_fence()).await?,
            LiveLedgerCommitOutcome::Committed {
                head: expected.reference.clone()
            }
        );
        assert_eq!(
            ops.load_live_head(fixture.session.id()).await?,
            Some(expected.clone())
        );
        assert_eq!(fixture.actor().await?, actor, "{backend:?}");
        let read = read_live_composite(
            ops,
            LiveCompositeReadRequest::new(fixture.session.id().clone(), None, 0, 64)?,
        )
        .await?
        .ok_or("composite")?;
        assert_eq!(read.authority().live_head(), Some(&expected.reference));
        assert_eq!(read.authority().actor(), &actor);
        assert_eq!(read.records().len(), 2);
        assert!(!read.has_more());
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_different_successors_have_one_winner_and_exact_replay() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let a = prepared(fixture.session.id(), None, vec![observation(1, "a")?])?;
        let b = prepared(fixture.session.id(), None, vec![observation(1, "b")?])?;
        let ops = fixture.ops()?;
        let (left, right) = tokio::join!(
            ops.commit_live_ledger(copy_prepared(&a), current_fence()),
            ops.commit_live_ledger(copy_prepared(&b), current_fence()),
        );
        let (left, right) = (left?, right?);
        let winner = match (&left, &right) {
            (
                LiveLedgerCommitOutcome::Committed { .. },
                LiveLedgerCommitOutcome::Conflict { .. },
            ) => a,
            (
                LiveLedgerCommitOutcome::Conflict { .. },
                LiveLedgerCommitOutcome::Committed { .. },
            ) => b,
            _ => return Err(format!("non-exclusive CAS: {backend:?} {left:?} {right:?}").into()),
        };
        assert!(matches!(
            ops.commit_live_ledger(winner, current_fence()).await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        assert_eq!(
            ops.load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?
                .reference
                .event_count,
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn same_key_or_head_does_not_substitute_for_exact_replay_bytes() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "original")?],
        )?;
        let mut replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        replay.records[0] = observation(1, "different")?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut replay = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "original")?],
        )?;
        replay.successor.payload.reserved.records = 1;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
    }
    Ok(())
}

#[tokio::test]
async fn alternate_predecessor_and_suffix_are_not_the_original_committed_operation() -> TestResult {
    let mut false_replays = Vec::new();
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
        let first_head = first.successor.clone();
        let never_committed = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "r1")?, observation(2, "r2")?],
        )?
        .successor;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let real = prepared(
            fixture.session.id(),
            Some(&first_head),
            vec![observation(2, "r2")?, observation(3, "r3")?],
        )?;
        let suffix = prepared(
            fixture.session.id(),
            Some(&never_committed),
            vec![observation(3, "r3")?],
        )?;
        assert_ne!(real.expected, suffix.expected);
        assert_eq!(real.successor, suffix.successor);
        assert!(suffix.encoded_records().is_ok());
        fixture
            .ops()?
            .commit_live_ledger(real, current_fence())
            .await?;
        let result = fixture
            .ops()?
            .commit_live_ledger(suffix, current_fence())
            .await?;
        if !matches!(result, LiveLedgerCommitOutcome::Conflict { .. }) {
            false_replays.push(format!("{backend:?}: {result:?}"));
        }
    }
    assert!(
        false_replays.is_empty(),
        "different operations treated as exact replay: {false_replays:?}"
    );
    Ok(())
}

#[tokio::test]
async fn exact_replay_binds_actor_expectation_and_quota_even_when_successor_matches() -> TestResult
{
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
        fixture
            .ops()?
            .commit_live_ledger(copy_prepared(&change), current_fence())
            .await?;
        let mut altered_actor = copy_prepared(&change);
        altered_actor.expected_actor = Some(fixture.actor().await?);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(altered_actor, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        let mut altered_quota = copy_prepared(&change);
        altered_quota.quota.encoded_bytes -= 1;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(altered_quota, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn oversized_corrupt_replay_rows_and_head_metadata_fail_before_blob_fetch() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for corruption in [
            "UPDATE runtime_live_events SET record=zeroblob(1048576)",
            "UPDATE runtime_live_events SET record_digest=zeroblob(1048576)",
            "UPDATE runtime_live_events SET channel_id=CAST(zeroblob(1048576) AS TEXT)",
            "UPDATE runtime_live_heads SET commit_digest=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET prefix_digest=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET transcript_snapshot=zeroblob(1048576)",
            "UPDATE runtime_live_heads SET request_snapshot=zeroblob(1048576)",
        ] {
            let fixture = Fixture::new(backend).await?;
            let change = prepared(fixture.session.id(), None, vec![observation(1, "r1")?])?;
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await?;
            let conn =
                meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
            conn.pragma_update(None, "ignore_check_constraints", "ON")?;
            conn.execute_batch(corruption)?;
            let result = fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await;
            assert!(
                matches!(result, Err(RuntimeStoreError::ReadFailed(ref error))
                if error.contains("bound") || error.contains("width")),
                "{backend:?}: {corruption}: {result:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn replay_cannot_omit_any_original_batch_member() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "a")?, observation(2, "b")?],
        )?;
        let mut replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        replay.records.pop();
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&replay), current_fence())
                .await
                .is_err()
        );
        replay.records.clear();
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_concurrent_replays_append_the_batch_once() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(
            fixture.session.id(),
            None,
            vec![observation(1, "a")?, observation(2, "b")?],
        )?;
        let ops = fixture.ops()?;
        let (left, right) = tokio::join!(
            ops.commit_live_ledger(copy_prepared(&change), current_fence()),
            ops.commit_live_ledger(change, current_fence()),
        );
        let results = [left?, right?];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, LiveLedgerCommitOutcome::Committed { .. }))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, LiveLedgerCommitOutcome::AlreadyCommitted { .. }))
                .count(),
            1
        );
        assert_eq!(
            ops.load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?
                .reference
                .event_count,
            2
        );
    }
    Ok(())
}

#[tokio::test]
async fn current_actor_identity_cannot_be_replaced_with_another_sessions_authority() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let other = Fixture::new(backend).await?;
        let mut change = prepared(fixture.session.id(), None, vec![observation(1, "a")?])?;
        change.expected_actor = Some(other.actor().await?);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict { .. }
        ));
        assert!(
            fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test]
async fn reserved_bytes_cannot_be_reused_by_unfunded_new_records() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let record = observation(1, "funded")?;
        let charge = LiveResourceCharge::for_event_record(&record.encode()?)?;
        let mut first = prepared(fixture.session.id(), None, vec![])?;
        first.successor.payload.reserved = charge;
        let quota = first.successor.payload.used.checked_add(charge)?;
        first.quota = quota;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut next = prepared(fixture.session.id(), Some(&head), vec![record])?;
        next.quota = quota;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&next), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        next.successor.payload.reserved = LiveResourceCharge::default();
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(head.payload.used, quota);
        assert_eq!(head.payload.reserved, LiveResourceCharge::default());
    }
    Ok(())
}

#[tokio::test]
async fn last_operation_receipt_has_explicit_once_per_head_storage_charge() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut change = prepared(fixture.session.id(), None, vec![])?;
        change.quota.encoded_bytes = crate::live_resources::LIVE_RECORD_STORAGE_ALLOWANCE_BYTES;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await
                .is_err()
        );
        change.quota.encoded_bytes += 32;
        assert_eq!(
            change.successor.payload.used.encoded_bytes,
            change.quota.encoded_bytes
        );
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        let first = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let next = prepared(fixture.session.id(), Some(&first), vec![])?;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let second = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(first.payload.used, second.payload.used);
    }
    Ok(())
}

#[tokio::test]
async fn snapshots_replace_their_charge_and_control_only_commits_are_fenced() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        first.successor.payload.transcript_snapshot = Arc::new(vec![17; 4096]);
        first.successor.payload.request_snapshot = Arc::new(vec![31; 1024]);
        first.successor.payload.used.encoded_bytes += 5120;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let old = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut next = prepared(fixture.session.id(), Some(&old), vec![])?;
        next.successor.payload.transcript_snapshot = Arc::new(vec![1]);
        next.successor.payload.request_snapshot = Arc::new(vec![2]);
        next.successor.payload.used.encoded_bytes -= 5118;
        next.successor.payload.ingress_generation += 1;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
        let new = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(new.payload.used.records, 1);
        assert_eq!(
            new.payload.used.encoded_bytes + 5118,
            old.payload.used.encoded_bytes
        );
        assert_eq!(new.reference.prefix_digest, old.reference.prefix_digest);
        assert_eq!(new.payload.ingress_generation, 2);
    }
    Ok(())
}

#[tokio::test]
async fn invalid_prepared_counters_prefix_and_scope_never_publish_partial_state() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        for defect in 0..13 {
            let mut change = prepared(
                fixture.session.id(),
                None,
                vec![observation(1, "one")?, observation(2, "two")?],
            )?;
            match defect {
                0 => change.successor.reference.event_count += 1,
                1 => {
                    change.successor.reference.prefix_digest =
                        LiveLedgerPrefixDigest::from_sha256([0; 32]);
                }
                2 => change.successor.reference.revision += 1,
                3 => change.successor.payload.used.records -= 1,
                4 => change.successor.payload.used.encoded_bytes -= 1,
                5 => change.successor.payload.used.encoded_bytes += 1,
                6 => {
                    change.successor.payload.reserved.encoded_bytes =
                        LIVE_LEDGER_MAX_CHARGE.encoded_bytes;
                }
                7 => change.successor.payload.ingress_generation = 0,
                8 => change.records[1] = observation(1, "duplicate")?,
                9 => change.records[1] = observation(3, "gap")?,
                10 => change.successor.reference.generation = 0,
                11 => change.successor.reference.generation = u64::MAX,
                12 => change.successor.payload.request_snapshot = Arc::new(vec![1]),
                _ => unreachable!(),
            }
            assert!(
                fixture
                    .ops()?
                    .commit_live_ledger(change, current_fence())
                    .await
                    .is_err(),
                "{backend:?} defect={defect}"
            );
            assert!(
                fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .is_none()
            );
        }
        let good = prepared(fixture.session.id(), None, vec![observation(1, "success")?])?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(good, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn external_epoch_fence_conflict_and_backoff_leave_no_head_or_events() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        for decision in [
            RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "revoked".into(),
            },
            RuntimeStoreWriteFenceOutcome::Backoff {
                reason: "unavailable".into(),
            },
        ] {
            let change = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
            let result = fixture
                .ops()?
                .commit_live_ledger(change, Arc::new(Fence(decision.clone())))
                .await;
            match decision {
                RuntimeStoreWriteFenceOutcome::Conflict { .. } => assert!(matches!(
                    result,
                    Err(RuntimeStoreError::WriteFenceConflict { .. })
                )),
                RuntimeStoreWriteFenceOutcome::Backoff { .. } => assert!(matches!(
                    result,
                    Err(RuntimeStoreError::WriteFenceBackoff { .. })
                )),
                RuntimeStoreWriteFenceOutcome::Applied => unreachable!(),
            }
            assert!(
                fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .is_none()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn actor_fence_is_exact_but_ordinary_live_append_commutes_with_actor_writes() -> TestResult {
    for backend in [
        Backend::Memory,
        #[cfg(feature = "sqlite-store")]
        Backend::WholeBlob,
    ] {
        let fixture = Fixture::new(backend).await?;
        let old_actor = fixture.actor().await?;
        let mut first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        first.expected_actor = Some(old_actor.clone());
        let replay = copy_prepared(&first);
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        assert_eq!(fixture.actor().await?, old_actor);
        let mut updated = fixture.session.clone();
        updated.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("ordinary turn"),
        ));
        save_actor(fixture.store.as_ref(), &updated).await?;
        let current_actor = fixture.actor().await?;
        assert_ne!(current_actor, old_actor);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(replay, current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let mut stale = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "next")?],
        )?;
        stale.expected_actor = Some(old_actor);
        assert_eq!(
            fixture
                .ops()?
                .commit_live_ledger(stale, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict {
                current: Some(current_actor.clone())
            }
        );
        let independent = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "next")?],
        )?;
        fixture
            .ops()?
            .commit_live_ledger(independent, current_fence())
            .await?;
        assert_eq!(fixture.actor().await?, current_actor);
    }
    Ok(())
}

#[tokio::test]
async fn missing_actor_cannot_acquire_a_live_head() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let missing = SessionId::new();
        let change = prepared(&missing, None, vec![observation(1, "one")?])?;
        assert_eq!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await?,
            LiveLedgerCommitOutcome::ActorConflict { current: None }
        );
        assert!(fixture.ops()?.load_live_head(&missing).await?.is_none());
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_late_insert_failure_rolls_back_head_events_and_snapshot_charges() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let first = prepared(fixture.session.id(), None, vec![observation(1, "one")?])?;
        fixture
            .ops()?
            .commit_live_ledger(first, current_fence())
            .await?;
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.execute_batch(
            "CREATE TABLE unrelated_owner (value TEXT NOT NULL);
             INSERT INTO unrelated_owner VALUES ('preserve');
             CREATE TRIGGER reject_third BEFORE INSERT ON runtime_live_events
             WHEN NEW.sequence=3 BEGIN SELECT RAISE(ABORT, 'injected late failure'); END;",
        )?;
        let next = prepared(
            fixture.session.id(),
            Some(&head),
            vec![observation(2, "two")?, observation(3, "three")?],
        )?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&next), current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(head)
        );
        let count: i64 = conn.query_row("SELECT count(*) FROM runtime_live_events", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);
        let foreign: String =
            conn.query_row("SELECT value FROM unrelated_owner", [], |row| row.get(0))?;
        assert_eq!(foreign, "preserve");
        conn.execute_batch("DROP TRIGGER reject_third")?;
        fixture
            .ops()?
            .commit_live_ledger(next, current_fence())
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_restart_replays_exact_payload_and_rejects_corrupted_replay() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let change = prepared(fixture.session.id(), None, vec![observation(1, "durable")?])?;
        let replay = copy_prepared(&change);
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        let reopened = match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&fixture.path)?,
            Backend::HeadCanonical => {
                crate::store::SqliteRuntimeStore::new_head_canonical(&fixture.path)?
            }
            Backend::Memory => unreachable!(),
        };
        assert_eq!(
            reopened.load_live_head(fixture.session.id()).await?,
            Some(replay.successor.clone())
        );
        assert!(matches!(
            reopened
                .commit_live_ledger(copy_prepared(&replay), current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let conn = meerkat_sqlite::open(&fixture.path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.execute(
            "UPDATE runtime_live_events SET record_digest=zeroblob(32)",
            [],
        )?;
        assert!(
            reopened
                .commit_live_ledger(replay, current_fence())
                .await
                .is_err()
        );
    }
    Ok(())
}
