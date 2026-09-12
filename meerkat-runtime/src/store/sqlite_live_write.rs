use crate::live_ledger::source::{
    LIVE_SOURCE_ROW_MAX_BYTES, LiveSourceChargeDelta, LiveSourceMutationCheck, LiveSourceRow,
    encoded_source_identity,
};
use crate::live_ledger::write::{
    LiveLedgerCommitOutcome, LiveLedgerPayloadState, LiveLedgerStoredHead, PreparedLiveLedgerCommit,
};
use crate::live_resources::{LIVE_LEDGER_MAX_CHARGE, LiveResourceCharge};

fn live_source_in_snapshot(
    tx: &Transaction<'_>,
    source: &meerkat_core::live_execution::request::LiveSourceKey,
) -> Result<Option<LiveSourceRow>, RuntimeStoreError> {
    let identity = encoded_source_identity(source)?;
    let row = tx
        .query_row(
            "SELECT CASE WHEN length(CAST(record AS BLOB)) <= ?4 THEN record END,
                CASE WHEN length(CAST(record_digest AS BLOB)) = 32 THEN record_digest END
         FROM runtime_live_sources WHERE session_id=?1 AND channel_id=?2 AND source_identity=?3",
            params![
                source.session_id().to_string(),
                source.channel_id().as_str(),
                identity,
                LIVE_SOURCE_ROW_MAX_BYTES
            ],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let Some((bytes, digest)) = row else {
        return Ok(None);
    };
    let (Some(bytes), Some(digest)) = (bytes, digest) else {
        return Err(RuntimeStoreError::ReadFailed(
            "live source row exceeds its accepted bound".into(),
        ));
    };
    let head = tx
        .query_row(
            "SELECT format_version,generation,revision,length(CAST(prefix_digest AS BLOB)),
                length(CAST(commit_digest AS BLOB)) FROM runtime_live_heads WHERE session_id=?1",
            [source.session_id().to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    if !matches!(head, Some((1, generation, revision, 32, 32)) if generation > 0 && revision > 0) {
        return Err(RuntimeStoreError::ReadFailed(
            "live source row has no valid versioned committed head".into(),
        ));
    }
    LiveSourceRow::restore(source.clone(), bytes, &digest).map(Some)
}

fn live_connection(
    path: &Path,
    profile: RuntimeSessionPersistenceProfile,
) -> Result<RuntimeConn, RuntimeStoreError> {
    match profile {
        RuntimeSessionPersistenceProfile::WholeBlobV1 => open_runtime_connection(path),
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
            open_head_canonical_runtime_connection(path)
        }
    }
}

fn live_stored_head_in_snapshot(
    tx: &Transaction<'_>,
    session_id: &meerkat_core::SessionId,
) -> Result<Option<LiveLedgerStoredHead>, RuntimeStoreError> {
    let Some(reference) = live_head_in_snapshot(tx, session_id)? else {
        return Ok(None);
    };
    let payload = tx
        .query_row(
            "SELECT used_records, used_bytes, reserved_records, reserved_bytes, ingress_generation,
                CASE WHEN length(CAST(transcript_snapshot AS BLOB)) + length(CAST(request_snapshot AS BLOB)) <= min(used_bytes, ?2)
                     THEN transcript_snapshot END,
                CASE WHEN length(CAST(transcript_snapshot AS BLOB)) + length(CAST(request_snapshot AS BLOB)) <= min(used_bytes, ?2)
                     THEN request_snapshot END
         FROM runtime_live_heads WHERE session_id = ?1",
            params![session_id.to_string(), LIVE_LEDGER_MAX_CHARGE.encoded_bytes],
            |row| {
                Ok((
                    LiveResourceCharge {
                        records: row.get(0)?,
                        encoded_bytes: row.get(1)?,
                    },
                    LiveResourceCharge {
                        records: row.get(2)?,
                        encoded_bytes: row.get(3)?,
                    },
                    row.get::<_, u64>(4)?,
                    row.get::<_, Option<Vec<u8>>>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                ))
            },
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let (used, reserved, ingress_generation, transcript, request) = payload;
    let (Some(transcript), Some(request)) = (transcript, request) else {
        return Err(RuntimeStoreError::ReadFailed(
            "live snapshots exceed the accounted byte bound".into(),
        ));
    };
    let head = LiveLedgerStoredHead {
        reference,
        payload: LiveLedgerPayloadState {
            used,
            reserved,
            ingress_generation,
            transcript_snapshot: Arc::new(transcript),
            request_snapshot: Arc::new(request),
        },
    };
    head.validate_payload()?;
    Ok(Some(head))
}

struct BoundedLiveReplayRow {
    channel: Option<String>,
    bytes: Option<Vec<u8>>,
    digest: Option<Vec<u8>>,
}

fn bounded_live_replay_row(
    tx: &Transaction<'_>,
    session_id: &str,
    sequence: u64,
) -> Result<BoundedLiveReplayRow, RuntimeStoreError> {
    tx.query_row(
        "SELECT
            CASE WHEN length(CAST(channel_id AS BLOB)) BETWEEN 1 AND 128 THEN channel_id END,
            CASE WHEN length(CAST(record AS BLOB)) <= ?3 THEN record END,
            CASE WHEN length(CAST(record_digest AS BLOB)) = 32 THEN record_digest END
         FROM runtime_live_events WHERE session_id = ?1 AND sequence = ?2",
        params![
            session_id,
            sequence,
            crate::store::live_read::LIVE_COMPOSITE_MAX_RECORD_BYTES
        ],
        |row| {
            Ok(BoundedLiveReplayRow {
                channel: row.get(0)?,
                bytes: row
                    .get::<_, Option<JsonColumnBytes>>(1)?
                    .map(JsonColumnBytes::into_bytes),
                digest: row.get(2)?,
            })
        },
    )
    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))
}

#[cfg(test)]
mod bounded_replay_tests {
    use super::*;

    #[test]
    fn oversized_columns_are_null_at_the_sql_result_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = Connection::open_in_memory()?;
        let tx = conn.transaction()?;
        // Corrupt physical columns without allocating their payloads in Rust.
        // This invokes the production query, not a replacement test query.
        tx.execute_batch(
            "CREATE TABLE runtime_live_events (session_id, sequence, channel_id, record, record_digest);
             INSERT INTO runtime_live_events VALUES
                 ('session', 1, CAST(zeroblob(1048576) AS TEXT), zeroblob(1048576), zeroblob(1048576));",
        )?;
        let row = bounded_live_replay_row(&tx, "session", 1)?;
        assert!(row.channel.is_none());
        assert!(row.bytes.is_none());
        assert!(row.digest.is_none());
        Ok(())
    }
}

fn commit_live_head_events_in_txn(
    tx: &Transaction<'_>,
    profile: RuntimeSessionPersistenceProfile,
    prepared: &PreparedLiveLedgerCommit,
    encoded: &[Vec<u8>],
) -> Result<LiveLedgerCommitOutcome, RuntimeStoreError> {
    let before = live_stored_head_in_snapshot(tx, prepared.session_id())?;
    let session_id = prepared.session_id().to_string();
    let operation = prepared.operation_digest(encoded)?;
    if before.as_ref() == Some(prepared.successor()) {
        let receipt: Option<Vec<u8>> = tx
            .query_row(
                "SELECT CASE WHEN length(CAST(commit_digest AS BLOB)) = 32 THEN commit_digest END
             FROM runtime_live_heads WHERE session_id = ?1",
                [&session_id],
                |row| row.get(0),
            )
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let receipt = receipt.ok_or_else(|| {
            RuntimeStoreError::ReadFailed("invalid live commit digest width".into())
        })?;
        if receipt != operation.0 {
            return Ok(LiveLedgerCommitOutcome::Conflict {
                current: before.map(|head| head.reference),
            });
        }
        for source in prepared.sources() {
            if live_source_in_snapshot(tx, source.replacement.source())?
                .as_ref()
                .is_none_or(|row| {
                    row.bytes() != source.replacement.bytes()
                        || row.digest() != source.replacement.digest()
                })
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "live source replay differs from committed content".into(),
                ));
            }
        }
        for ((record, bytes), witness) in prepared
            .records()
            .iter()
            .zip(encoded)
            .zip(prepared.prefix_witnesses(encoded))
        {
            let stored = bounded_live_replay_row(tx, &session_id, record.sequence().get())?;
            let BoundedLiveReplayRow {
                channel: Some(channel),
                bytes: Some(stored_bytes),
                digest: Some(digest),
            } = stored
            else {
                return Err(RuntimeStoreError::ReadFailed(
                    "live replay row exceeds its accepted bound".into(),
                ));
            };
            if channel != record.channel_id().as_str()
                || stored_bytes != *bytes
                || digest != Sha256::digest(bytes).as_slice()
                || live_event_witness(tx, prepared.session_id(), record.sequence().get())?
                    != Some(witness)
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "live replay bytes differ".into(),
                ));
            }
        }
        return Ok(LiveLedgerCommitOutcome::AlreadyCommitted {
            head: prepared.successor().reference.clone(),
        });
    }
    if before.as_ref().map(|head| &head.reference) != prepared.expected()
        || before
            .as_ref()
            .is_some_and(|head| head.reference == prepared.successor().reference)
    {
        return Ok(LiveLedgerCommitOutcome::Conflict {
            current: before.map(|head| head.reference),
        });
    }
    let runtime_id = LogicalRuntimeId::for_session(prepared.session_id());
    let actor = match profile {
        RuntimeSessionPersistenceProfile::WholeBlobV1 => {
            load_whole_blob_store_authority(tx, &runtime_id)?
                .map(RuntimeSessionAuthority::WholeBlob)
        }
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
            load_head_canonical_authority(tx, &runtime_id)?
        }
    };
    if actor.is_none()
        || prepared
            .expected_actor()
            .is_some_and(|expected| Some(expected) != actor.as_ref())
    {
        return Ok(LiveLedgerCommitOutcome::ActorConflict { current: actor });
    }
    let mut source_charges = LiveSourceChargeDelta::default();
    for source in prepared.sources() {
        let current = live_source_in_snapshot(tx, source.replacement.source())?;
        prepared.validate_new_source_context(source, current.as_ref())?;
        match source.check(current.as_ref())? {
            LiveSourceMutationCheck::Match(delta) => {
                source_charges = source_charges.checked_add(delta)?;
            }
            LiveSourceMutationCheck::Conflict { current } => {
                return Ok(LiveLedgerCommitOutcome::SourceConflict {
                    source: source.replacement.source().clone(),
                    current,
                });
            }
        }
    }
    prepared.validate(before.as_ref(), encoded, source_charges)?;
    let head = prepared.successor();
    tx.execute(
        "INSERT INTO runtime_live_heads (
            session_id, format_version, generation, revision, event_count, prefix_digest,
            used_records, used_bytes, reserved_records, reserved_bytes, ingress_generation,
            transcript_snapshot, request_snapshot, commit_digest
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(session_id) DO UPDATE SET
            generation=excluded.generation, revision=excluded.revision,
            event_count=excluded.event_count, prefix_digest=excluded.prefix_digest,
            used_records=excluded.used_records, used_bytes=excluded.used_bytes,
            reserved_records=excluded.reserved_records, reserved_bytes=excluded.reserved_bytes,
            ingress_generation=excluded.ingress_generation,
            transcript_snapshot=excluded.transcript_snapshot, request_snapshot=excluded.request_snapshot,
            commit_digest=excluded.commit_digest",
        params![
            session_id, head.reference.generation, head.reference.revision, head.reference.event_count,
            head.reference.prefix_digest.as_bytes().as_slice(), head.payload.used.records,
            head.payload.used.encoded_bytes, head.payload.reserved.records, head.payload.reserved.encoded_bytes,
            head.payload.ingress_generation, head.payload.transcript_snapshot.as_slice(),
            head.payload.request_snapshot.as_slice(),
            operation.0.as_slice(),
        ],
    ).map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    for ((record, bytes), witness) in prepared
        .records()
        .iter()
        .zip(encoded)
        .zip(prepared.prefix_witnesses(encoded))
    {
        tx.execute(
            "INSERT INTO runtime_live_events (session_id, sequence, channel_id, record, record_digest, commit_revision, prefix_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![session_id, record.sequence().get(), record.channel_id().as_str(),
                bytes, Sha256::digest(bytes).as_slice(), witness.commit_revision, witness.prefix.as_bytes().as_slice()],
        ).map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    }
    for source in prepared.sources() {
        let row = &source.replacement;
        tx.execute(
            "INSERT INTO runtime_live_sources (session_id, channel_id, source_identity, record, record_digest)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(session_id,channel_id,source_identity) DO UPDATE
             SET record=excluded.record, record_digest=excluded.record_digest",
            params![row.source().session_id().to_string(), row.source().channel_id().as_str(),
                encoded_source_identity(row.source())?, row.bytes(), row.digest().as_bytes().as_slice()],
        ).map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    }
    Ok(LiveLedgerCommitOutcome::Committed {
        head: head.reference.clone(),
    })
}
