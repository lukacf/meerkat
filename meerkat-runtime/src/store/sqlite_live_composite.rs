use crate::live_ledger::record::{LiveEventPrefixWitness, LiveLedgerRecord};
use crate::live_ledger::transcript::{
    LiveHeadReference, LiveLedgerFormatV1, LiveLedgerPrefixDigest,
};
use crate::store::live_history::{
    LiveHistoryReadError, LiveHistoryReadRequest, LiveHistoryWindow, validate_retained_prefix,
};
use crate::store::live_read::{
    LiveCompositeCapture, LiveCompositeReadProfile, LiveCompositeReadRequest, RuntimeLiveLedgerOps,
    bounded_live_window,
};
use sha2::{Digest, Sha256};

fn live_event_witness(
    tx: &Transaction<'_>,
    session_id: &meerkat_core::SessionId,
    sequence: u64,
) -> Result<Option<LiveEventPrefixWitness>, RuntimeStoreError> {
    let row = tx.query_row(
        "SELECT commit_revision, CASE WHEN length(CAST(prefix_digest AS BLOB))=32 THEN prefix_digest END
         FROM runtime_live_events WHERE session_id=?1 AND sequence=?2",
        params![session_id.to_string(), sequence],
        |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
    ).optional().map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    row.map(|(commit_revision, prefix)| {
        if commit_revision == 0 {
            return Err(RuntimeStoreError::ReadFailed(
                "invalid live event commit revision".into(),
            ));
        }
        let prefix = prefix.ok_or_else(|| {
            RuntimeStoreError::ReadFailed("invalid live event prefix width".into())
        })?;
        Ok(LiveEventPrefixWitness {
            commit_revision,
            prefix: LiveLedgerPrefixDigest::from_sha256(prefix.try_into().map_err(|_| {
                RuntimeStoreError::ReadFailed("invalid live event prefix width".into())
            })?),
        })
    })
    .transpose()
}

fn live_head_in_snapshot(
    tx: &Transaction<'_>,
    session_id: &meerkat_core::SessionId,
) -> Result<Option<LiveHeadReference>, RuntimeStoreError> {
    let row = tx
        .query_row(
            "SELECT format_version, generation, revision, event_count,
                    CASE WHEN length(CAST(prefix_digest AS BLOB)) = 32 THEN prefix_digest END
         FROM runtime_live_heads WHERE session_id = ?1",
            [session_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let (count, first, last): (i64, i64, i64) = tx
        .query_row(
            "SELECT count(*), coalesce(min(sequence),0), coalesce(max(sequence),0)
         FROM runtime_live_events WHERE session_id = ?1",
            [session_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let Some((format, generation, revision, event_count, digest)) = row else {
        let sources_exist: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_live_sources WHERE session_id=?1)",
                [session_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        if count != 0 || sources_exist {
            return Err(RuntimeStoreError::ReadFailed(
                "live records or sources exist without a committed head".into(),
            ));
        }
        return Ok(None);
    };
    if format != 1 || generation <= 0 || revision <= 0 || event_count < 0 {
        return Err(RuntimeStoreError::ReadFailed(
            "unsupported or malformed live head".into(),
        ));
    }
    let digest = digest.ok_or_else(|| {
        RuntimeStoreError::ReadFailed("live head digest has invalid width".into())
    })?;
    if count != event_count || last != event_count || first != i64::from(event_count != 0) {
        return Err(RuntimeStoreError::ReadFailed(
            "live head and immutable record prefix disagree".into(),
        ));
    }
    let head = LiveHeadReference {
        format: LiveLedgerFormatV1::V1,
        session_id: session_id.clone(),
        generation: u64::try_from(generation)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?,
        revision: u64::try_from(revision)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?,
        event_count: u64::try_from(event_count)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?,
        prefix_digest: LiveLedgerPrefixDigest::from_sha256(digest.try_into().map_err(|_| {
            RuntimeStoreError::ReadFailed("live head digest has invalid width".into())
        })?),
    };
    let prefix = if head.event_count == 0 {
        LiveLedgerPrefixDigest::empty(session_id, head.generation)
    } else {
        let witness = live_event_witness(tx, session_id, head.event_count)?.ok_or_else(|| {
            RuntimeStoreError::ReadFailed("missing live head event witness".into())
        })?;
        if witness.commit_revision > head.revision {
            return Err(RuntimeStoreError::ReadFailed(
                "live event belongs to a future commit".into(),
            ));
        }
        witness.prefix
    };
    if prefix != head.prefix_digest {
        return Err(RuntimeStoreError::ReadFailed(
            "live event prefix witness differs from its committed head".into(),
        ));
    }
    Ok(Some(head))
}

fn live_window_in_snapshot(
    tx: &Transaction<'_>,
    request: &LiveCompositeReadRequest,
    head: Option<&LiveHeadReference>,
) -> Result<(Vec<LiveLedgerRecord>, bool), RuntimeStoreError> {
    let end = head.map_or(0, |head| head.event_count);
    if request.after_sequence() > end {
        return Err(RuntimeStoreError::ReadFailed(
            "live composite window exceeds its captured head".into(),
        ));
    }
    // Neither index nor payload metadata is trusted before validation. Include
    // candidates named by either side so corruption cannot hide itself through
    // a filter. Unknown/oversized shapes remain selected to fail explicitly.
    let mut statement = tx.prepare(
        "WITH live_window AS (
             SELECT sequence,
                    CASE WHEN length(CAST(channel_id AS BLOB)) BETWEEN 1 AND 128 THEN channel_id END AS channel_id,
                    CASE WHEN length(CAST(record_digest AS BLOB)) = 32 THEN record_digest END AS record_digest,
                    commit_revision,
                    CASE WHEN length(CAST(record AS BLOB)) <= ?5 THEN record ELSE NULL END AS encoded,
                    CASE WHEN length(CAST(record AS BLOB)) <= ?5 AND json_valid(record)
                         THEN coalesce(json_extract(record, '$.record.channel_id'),
                                       json_extract(record, '$.channel_id')) END AS canonical_channel
             FROM runtime_live_events
             WHERE session_id = ?1 AND sequence > ?2 AND sequence <= ?3
         )
         SELECT sequence, channel_id, encoded, record_digest, commit_revision FROM live_window
         WHERE ?4 IS NULL OR channel_id = ?4 OR canonical_channel = ?4 OR canonical_channel IS NULL
               OR channel_id IS NULL OR record_digest IS NULL
         ORDER BY sequence",
    ).map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let rows = statement
        .query_map(
            params![
                request.session_id().to_string(),
                request.after_sequence(),
                end,
                request
                    .channel_id()
                    .map(meerkat_core::live_execution::LiveChannelId::as_str),
                crate::store::live_read::LIVE_COMPOSITE_MAX_RECORD_BYTES
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<JsonColumnBytes>>(2)?
                        .map(JsonColumnBytes::into_bytes),
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    bounded_live_window(
        request,
        rows.map(|row| {
            let (sequence, channel, bytes, digest, commit_revision) =
                row.map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            if commit_revision == 0 || head.is_none_or(|head| commit_revision > head.revision) {
                return Err(RuntimeStoreError::ReadFailed(
                    "live record belongs to another commit prefix".into(),
                ));
            }
            let channel = channel.ok_or_else(|| {
                RuntimeStoreError::ReadFailed("invalid live record channel width".into())
            })?;
            let digest = digest.ok_or_else(|| {
                RuntimeStoreError::ReadFailed("invalid live record digest width".into())
            })?;
            let bytes = bytes.ok_or_else(|| {
                RuntimeStoreError::ReadFailed(
                    "stored live record exceeds its accepted byte bound".into(),
                )
            })?;
            if Sha256::digest(&bytes).as_slice() != digest {
                return Err(RuntimeStoreError::ReadFailed(
                    "live record digest mismatch".into(),
                ));
            }
            let record: LiveLedgerRecord = serde_json::from_slice(&bytes).map_err(|error| {
                RuntimeStoreError::ReadFailed(format!("invalid live ledger record: {error}"))
            })?;
            if record.sequence().get() != sequence
                || record.channel_id().as_str() != channel
                || record
                    .encode()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                    != bytes
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "live record index or canonical encoding mismatch".into(),
                ));
            }
            let witness =
                live_event_witness(tx, request.session_id(), sequence)?.ok_or_else(|| {
                    RuntimeStoreError::ReadFailed("missing live event prefix witness".into())
                })?;
            let previous = if sequence == 1 {
                None
            } else {
                live_event_witness(tx, request.session_id(), sequence - 1)?
            };
            let head = head
                .ok_or_else(|| RuntimeStoreError::ReadFailed("live record has no head".into()))?;
            witness.validate_record(
                previous,
                request.session_id(),
                head.generation,
                record.sequence(),
                &bytes,
            )?;
            Ok(record)
        }),
    )
}

#[async_trait::async_trait]
impl RuntimeLiveLedgerOps for SqliteRuntimeStore {
    fn composite_read_profile(&self) -> LiveCompositeReadProfile {
        LiveCompositeReadProfile::AtomicSnapshot
    }

    fn ledger_write_profile(&self) -> crate::store::live_read::LiveLedgerWriteProfile {
        crate::store::live_read::LiveLedgerWriteProfile::AtomicHeadEventsSources
    }

    async fn read_live_history(
        &self,
        request: &LiveHistoryReadRequest,
    ) -> Result<LiveHistoryWindow, LiveHistoryReadError> {
        let path = self.path.clone();
        let profile = self.session_persistence_profile;
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = live_connection(&path, profile)?;
            let tx = conn
                .transaction()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let sid = &request.head().session_id;
            let current =
                live_head_in_snapshot(&tx, sid)?.ok_or(LiveHistoryReadError::SnapshotExpired)?;
            let end = request.head().event_count;
            let last = if end == 0 {
                None
            } else {
                live_event_witness(&tx, sid, end)?
            };
            let next = if end < current.event_count {
                live_event_witness(&tx, sid, end + 1)?
            } else {
                None
            };
            validate_retained_prefix(request.head(), &current, last, next)?;
            let (records, more) =
                live_window_in_snapshot(&tx, request.selection(), Some(request.head()))?;
            LiveHistoryWindow::new(&request, records, more)
        })
        .await
        .map_err(|error| {
            RuntimeStoreError::Internal(format!("live history task failed: {error}"))
        })?
    }

    async fn lookup_live_source(
        &self,
        source: &meerkat_core::live_execution::request::LiveSourceKey,
    ) -> Result<Option<LiveSourceRow>, RuntimeStoreError> {
        crate::live_ledger::source::validate_source_storage_key(source)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let path = self.path.clone();
        let profile = self.session_persistence_profile;
        let source = source.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = live_connection(&path, profile)?;
            let tx = conn
                .transaction()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            live_source_in_snapshot(&tx, &source)
        })
        .await
        .map_err(|error| {
            RuntimeStoreError::Internal(format!("live source read task failed: {error}"))
        })?
    }

    async fn load_live_head(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<LiveLedgerStoredHead>, RuntimeStoreError> {
        let path = self.path.clone();
        let profile = self.session_persistence_profile;
        let session_id = session_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = live_connection(&path, profile)?;
            let tx = conn
                .transaction()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            live_stored_head_in_snapshot(&tx, &session_id)
        })
        .await
        .map_err(|error| {
            RuntimeStoreError::Internal(format!("live head read task failed: {error}"))
        })?
    }

    async fn commit_live_ledger(
        &self,
        prepared: PreparedLiveLedgerCommit,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<LiveLedgerCommitOutcome, RuntimeStoreError> {
        let path = self.path.clone();
        let profile = self.session_persistence_profile;
        tokio::task::spawn_blocking(move || {
            let encoded = prepared.encoded_records()?;
            let mut conn = live_connection(&path, profile)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            let outcome = commit_live_head_events_in_txn(&tx, profile, &prepared, &encoded)?;
            if matches!(outcome, LiveLedgerCommitOutcome::Committed { .. }) {
                commit_runtime_transaction(tx, Some(write_fence.as_ref()))?;
            }
            Ok(outcome)
        })
        .await
        .map_err(|error| {
            RuntimeStoreError::Internal(format!("live head write task failed: {error}"))
        })?
    }

    async fn capture_live_composite(
        &self,
        request: &LiveCompositeReadRequest,
    ) -> Result<Option<LiveCompositeCapture>, RuntimeStoreError> {
        let path = self.path.clone();
        let profile = self.session_persistence_profile;
        let request = request.clone();
        #[cfg(any(test, feature = "test-support"))]
        let pause = self.live_composite_test_pause.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = match profile {
                RuntimeSessionPersistenceProfile::WholeBlobV1 => open_runtime_connection(&path)?,
                RuntimeSessionPersistenceProfile::HeadCanonicalV1 => open_head_canonical_runtime_connection(&path)?,
            };
            let tx = conn.transaction().map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let runtime_id = LogicalRuntimeId::for_session(request.session_id());
            let head = live_head_in_snapshot(&tx, request.session_id())?;
            #[cfg(any(test, feature = "test-support"))]
            if let Some(pause) = &pause {
                pause.wait()?;
            }
            let captured = match profile {
                RuntimeSessionPersistenceProfile::WholeBlobV1 => {
                    let Some(actor) = load_whole_blob_store_authority(&tx, &runtime_id)? else {
                        if head.is_some() {
                            return Err(RuntimeStoreError::ReadFailed("live head has no committed actor".into()));
                        }
                        return Ok(None);
                    };
                    let bytes = tx.query_row(
                        "SELECT session_snapshot FROM runtime_whole_blob_bodies WHERE blob_sha256 = ?1",
                        [actor.blob_sha256()],
                        |row| Ok(row.get::<_, JsonColumnBytes>(0)?.into_bytes()),
                    ).map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let (records, more) = live_window_in_snapshot(&tx, &request, head.as_ref())?;
                    tx.rollback().map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    LiveCompositeCapture::whole_blob(Arc::new(bytes), actor, head, records, more)?
                },
                RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
                    let Some(actor) = load_head_canonical_authority(&tx, &runtime_id)? else {
                        if head.is_some() {
                            return Err(RuntimeStoreError::ReadFailed("live head has no committed actor".into()));
                        }
                        return Ok(None);
                    };
                    let actor = actor.head_canonical().ok_or_else(|| RuntimeStoreError::ReadFailed("head-canonical profile returned another authority kind".into()))?.clone();
                    let materialized = meerkat_store::sqlite_store::verify_runtime_boundary_head_canonical_in_txn(&tx, actor.boundary_head())
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    let (records, more) = live_window_in_snapshot(&tx, &request, head.as_ref())?;
                    tx.rollback().map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                    LiveCompositeCapture::head_canonical(materialized, actor, head, records, more)?
                },
            };
            Ok(Some(captured))
        }).await.map_err(|error| RuntimeStoreError::Internal(format!("live composite read task failed: {error}")))?
    }
}
