#[cfg(all(test, not(target_arch = "wasm32")))]
use crate::live_ledger::record::LiveEventPrefixWitness;
use crate::live_ledger::record::StoredLiveLedgerEvent;
use crate::live_ledger::source::{LiveSourceChargeDelta, LiveSourceMutationCheck, LiveSourceRow};
use crate::live_ledger::write::{
    LiveLedgerCommitOutcome, LiveLedgerStoredHead, PreparedLiveLedgerCommit, StoredLiveLedgerCommit,
};
use crate::store::live_history::{
    LiveHistoryReadError, LiveHistoryReadRequest, LiveHistoryWindow, validate_retained_prefix,
};
use crate::store::live_read::LiveLedgerWriteProfile;
use crate::store::live_read::{
    LiveCompositeCapture, LiveCompositeReadProfile, LiveCompositeReadRequest, RuntimeLiveLedgerOps,
    bounded_live_window,
};

fn enforce_memory_live_lifecycle_version(
    inner: &Inner,
    runtime_id: &LogicalRuntimeId,
    expected: Option<&super::super::MachineLifecycleExpectedVersion>,
) -> Result<(), RuntimeStoreError> {
    use super::super::{MachineLifecycleExpectedVersion, MachineLifecycleObservationVersion};
    let matches = match expected {
        None => true,
        Some(MachineLifecycleExpectedVersion::Missing) => {
            !inner.runtime_lifecycle.contains_key(&runtime_id.0)
        }
        Some(MachineLifecycleExpectedVersion::Version(expected)) => inner
            .runtime_lifecycle
            .get(&runtime_id.0)
            .is_some_and(|bytes| {
                MachineLifecycleObservationVersion::from_raw_record(bytes) == *expected
            }),
    };
    if !matches {
        return Err(RuntimeStoreError::MachineLifecycleVersionConflict {
            runtime_id: runtime_id.0.clone(),
        });
    }
    Ok(())
}

fn validate_memory_live_prefix(
    inner: &Inner,
    session_id: &meerkat_core::SessionId,
) -> Result<(), RuntimeStoreError> {
    let head = inner.live_heads.get(session_id);
    if head.is_none()
        && inner
            .live_sources
            .get(session_id)
            .is_some_and(|sources| !sources.is_empty())
    {
        return Err(RuntimeStoreError::ReadFailed(
            "live sources exist without a committed head".into(),
        ));
    }
    let records = inner.live_records.get(session_id);
    let end = head.map_or(0, |head| head.head.reference.event_count);
    let count = records.map_or(0, |records| records.len() as u64);
    let first = records
        .and_then(|records| records.first_key_value())
        .map_or(0, |(key, _)| *key);
    let last = records
        .and_then(|records| records.last_key_value())
        .map_or(0, |(key, _)| *key);
    if count != end || last != end || first != u64::from(end != 0) {
        return Err(RuntimeStoreError::ReadFailed(
            "live head and immutable record prefix disagree".into(),
        ));
    }
    if let Some(head) = head {
        let prefix = records.and_then(|rows| rows.last_key_value()).map_or_else(
            || {
                crate::live_ledger::transcript::LiveLedgerPrefixDigest::empty(
                    session_id,
                    head.head.reference.generation,
                )
            },
            |(_, event)| event.witness.prefix,
        );
        if prefix != head.head.reference.prefix_digest
            || records
                .and_then(|rows| rows.last_key_value())
                .is_some_and(|(_, event)| {
                    event.witness.commit_revision == 0
                        || event.witness.commit_revision > head.head.reference.revision
                })
        {
            return Err(RuntimeStoreError::ReadFailed(
                "live event prefix witness differs from its committed head".into(),
            ));
        }
    }
    Ok(())
}

fn memory_live_source<'a>(
    inner: &'a Inner,
    source: &meerkat_core::live_execution::request::LiveSourceKey,
) -> Option<&'a LiveSourceRow> {
    inner
        .live_sources
        .get(source.session_id())
        .and_then(|sources| sources.get(source))
}

fn memory_live_window(
    inner: &Inner,
    request: &LiveCompositeReadRequest,
    head: Option<&crate::live_ledger::transcript::LiveHeadReference>,
) -> Result<(Vec<crate::live_ledger::record::LiveLedgerRecord>, bool), RuntimeStoreError> {
    let end = head.map_or(0, |head| head.event_count);
    let revision = head.map_or(0, |head| head.revision);
    let rows = inner
        .live_records
        .get(request.session_id())
        .into_iter()
        .flat_map(|records| records.iter())
        .map(|(sequence, event)| {
            if *sequence != event.record.sequence().get()
                || (*sequence <= end
                    && (event.witness.commit_revision == 0
                        || event.witness.commit_revision > revision))
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "live record index or commit witness disagrees with its captured prefix".into(),
                ));
            }
            Ok(event)
        })
        .filter(|candidate| match candidate {
            Ok(event) => {
                event.record.sequence().get() > request.after_sequence()
                    && event.record.sequence().get() <= end
                    && request
                        .channel_id()
                        .is_none_or(|channel| channel == event.record.channel_id())
            }
            Err(_) => true,
        })
        .map(|candidate| {
            let event = candidate?;
            let sequence = event.record.sequence();
            let previous = inner
                .live_records
                .get(request.session_id())
                .and_then(|rows| rows.get(&(sequence.get() - 1)))
                .map(|event| event.witness);
            let bytes = event
                .record
                .encode()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let head = head
                .ok_or_else(|| RuntimeStoreError::ReadFailed("live record has no head".into()))?;
            event.witness.validate_record(
                previous,
                request.session_id(),
                head.generation,
                sequence,
                &bytes,
            )?;
            Ok(event.record.clone())
        });
    bounded_live_window(request, rows)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::live_ledger::transcript::{
        LiveHeadReference, LiveLedgerFormatV1, LiveLedgerPrefixDigest,
    };
    use crate::store::live_read::{LiveCompositeReadRequest, read_live_composite};

    fn stored_head(reference: LiveHeadReference) -> StoredLiveLedgerCommit {
        StoredLiveLedgerCommit {
            operation: crate::live_ledger::write::LiveLedgerCommitDigest([0; 32]),
            head: LiveLedgerStoredHead {
                reference,
                payload: crate::live_ledger::write::LiveLedgerPayloadState {
                    used: crate::live_resources::LiveResourceCharge::default(),
                    reserved: crate::live_resources::LiveResourceCharge::default(),
                    ingress_generation: 1,
                    transcript_snapshot: Arc::new(Vec::new()),
                    request_snapshot: Arc::new(Vec::new()),
                },
            },
        }
    }

    #[tokio::test]
    async fn orphan_source_rows_fail_before_absent_memory_head_or_actor_returns()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = InMemoryRuntimeStore::new();
        let session_id = meerkat_core::SessionId::new();
        let entry = serde_json::from_value(serde_json::json!({
            "kind":"cancellation_only",
            "intent":{
                "source":{"session_id":session_id,"channel_id":"voice",
                    "source":{"kind":"client_delegation","delegation":"cancel-first"}},
                "reason":"operator_requested"
            }
        }))?;
        let row = LiveSourceRow::encode(&entry)?;
        let source = row.source().clone();
        store
            .inner
            .lock()
            .await
            .live_sources
            .entry(session_id.clone())
            .or_default()
            .insert(source.clone(), row);
        assert!(store.lookup_live_source(&source).await.is_err());
        assert!(store.load_live_head(&session_id).await.is_err());
        assert!(
            read_live_composite(
                &store,
                LiveCompositeReadRequest::new(session_id, None, 0, 64)?
            )
            .await
            .is_err()
        );
        Ok(())
    }

    fn observation(
        sequence: u64,
        session_id: &meerkat_core::SessionId,
    ) -> Result<StoredLiveLedgerEvent, Box<dyn std::error::Error>> {
        let fit = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::check_record_fit(
            meerkat_contracts::wire::live_observation::LiveObservationRecord {
                sequence: meerkat_core::live_observation::LiveObservationSeq::new(sequence)?,
                channel_id: meerkat_core::live_execution::LiveChannelId::new("wanted"),
                observation: meerkat_core::live_observation::LiveTranscriptObservation::new(
                    meerkat_core::live_observation::LiveTranscriptDirection::Input,
                    meerkat_core::live_observation::LiveTranscriptRange::new(0.0, 1.0)?, "retained",
                ),
            },
        )?;
        let record = crate::live_ledger::record::LiveLedgerRecord::Observation(
            crate::live_ledger::transcript::StoredLiveObservation::from_fit(&fit),
        );
        let prefix = LiveLedgerPrefixDigest::empty(session_id, 1)
            .appended(record.sequence(), &record.encode()?);
        Ok(StoredLiveLedgerEvent {
            record,
            witness: LiveEventPrefixWitness {
                commit_revision: 1,
                prefix,
            },
        })
    }

    #[tokio::test]
    async fn corrupt_payload_sequence_cannot_hide_before_memory_index_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = InMemoryRuntimeStore::new();
        let session = meerkat_core::Session::new();
        let sid = session.id().clone();
        store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&sid),
                super::super::SerializedSessionSnapshot {
                    session_snapshot: Arc::new(serde_json::to_vec(&session)?),
                },
            )
            .await?;
        let mut inner = store.inner.lock().await;
        inner.live_heads.insert(
            sid.clone(),
            stored_head(LiveHeadReference {
                format: LiveLedgerFormatV1::V1,
                session_id: sid.clone(),
                generation: 1,
                revision: 1,
                event_count: 1,
                prefix_digest: observation(2, &sid)?.witness.prefix,
            }),
        );
        inner
            .live_records
            .insert(sid.clone(), BTreeMap::from([(1, observation(2, &sid)?)]));
        drop(inner);
        let result = read_live_composite(
            &store,
            LiveCompositeReadRequest::new(
                sid,
                Some(meerkat_core::live_execution::LiveChannelId::new("wanted")),
                0,
                64,
            )?,
        )
        .await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn memory_orphan_records_fail_before_missing_actor_or_head_returns()
    -> Result<(), Box<dyn std::error::Error>> {
        for keep_actor in [false, true] {
            let store = InMemoryRuntimeStore::new();
            let session = meerkat_core::Session::new();
            let sid = session.id().clone();
            if keep_actor {
                store
                    .commit_session_snapshot(
                        &LogicalRuntimeId::for_session(&sid),
                        super::super::SerializedSessionSnapshot {
                            session_snapshot: Arc::new(serde_json::to_vec(&session)?),
                        },
                    )
                    .await?;
            }
            store
                .inner
                .lock()
                .await
                .live_records
                .insert(sid.clone(), BTreeMap::from([(1, observation(1, &sid)?)]));
            let result =
                read_live_composite(&store, LiveCompositeReadRequest::new(sid, None, 0, 64)?).await;
            assert!(result.is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn queued_writer_cannot_split_a_memory_composite_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let session = meerkat_core::Session::new();
        let sid = session.id().clone();
        let rid = LogicalRuntimeId::for_session(&sid);
        store
            .commit_session_snapshot(
                &rid,
                super::super::SerializedSessionSnapshot {
                    session_snapshot: Arc::new(serde_json::to_vec(&session)?),
                },
            )
            .await?;
        let fit = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::check_record_fit(
                meerkat_contracts::wire::live_observation::LiveObservationRecord {
                    sequence: meerkat_core::live_observation::LiveObservationSeq::new(1)?,
                    channel_id: meerkat_core::live_execution::LiveChannelId::new("voice"),
                    observation: meerkat_core::live_observation::LiveTranscriptObservation::new(
                        meerkat_core::live_observation::LiveTranscriptDirection::Input,
                        meerkat_core::live_observation::LiveTranscriptRange::new(0.0, 1.0)?,
                        "retained",
                    ),
                },
            )?;
        let record = crate::live_ledger::record::LiveLedgerRecord::Observation(
            crate::live_ledger::transcript::StoredLiveObservation::from_fit(&fit),
        );
        let prefix =
            LiveLedgerPrefixDigest::empty(&sid, 1).appended(record.sequence(), &record.encode()?);
        store.inner.lock().await.live_records.insert(
            sid.clone(),
            BTreeMap::from([(
                1,
                StoredLiveLedgerEvent {
                    record,
                    witness: LiveEventPrefixWitness {
                        commit_revision: 1,
                        prefix,
                    },
                },
            )]),
        );
        store.inner.lock().await.live_heads.insert(
            sid.clone(),
            stored_head(LiveHeadReference {
                format: LiveLedgerFormatV1::V1,
                session_id: sid.clone(),
                generation: 1,
                revision: 1,
                event_count: 1,
                prefix_digest: prefix,
            }),
        );
        let mut successor = session.clone();
        successor.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("successor"),
        ));
        let bytes = Arc::new(serde_json::to_vec(&successor)?);
        let decoded = meerkat_core::Session::decode_whole_blob_document(&bytes)?;
        let authority = super::super::WholeBlobStoreAuthority::issued(
            sid.clone(),
            2,
            decoded.row_sha256_token().to_owned(),
        )?;
        let held = store.inner.lock().await;
        let (read_started_tx, read_started_rx) = tokio::sync::oneshot::channel();
        let reader_store = Arc::clone(&store);
        let request = LiveCompositeReadRequest::new(sid.clone(), None, 0, 64)?;
        let reading = tokio::spawn(async move {
            read_started_tx.send(()).map_err(|()| {
                RuntimeStoreError::ReadFailed("memory read fixture observer dropped".into())
            })?;
            read_live_composite(reader_store.as_ref(), request).await
        });
        read_started_rx.await?;
        let (write_started_tx, write_started_rx) = tokio::sync::oneshot::channel();
        let writer_store = Arc::clone(&store);
        let writing = tokio::spawn(async move {
            write_started_tx
                .send(())
                .map_err(|()| "memory write fixture observer dropped")?;
            let mut inner = writer_store.inner.lock().await;
            inner.sessions.insert(rid.0.clone(), bytes);
            inner.session_authorities.insert(rid.0, authority);
            let head = inner.live_heads.get_mut(&sid).ok_or("live head missing")?;
            head.head.reference.revision = 2;
            Ok::<_, &'static str>(())
        });
        write_started_rx.await?;
        drop(held);
        let captured = reading.await??.ok_or("captured read missing")?;
        writing.await?.map_err(std::io::Error::other)?;
        assert_eq!(captured.authority().actor().store_revision(), 1);
        assert_eq!(captured.authority().live_head().ok_or("head")?.revision, 1);
        assert_eq!(captured.session().messages(), session.messages());
        assert_eq!(captured.records().len(), 1);
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl RuntimeLiveLedgerOps for InMemoryRuntimeStore {
    fn composite_read_profile(&self) -> LiveCompositeReadProfile {
        LiveCompositeReadProfile::AtomicSnapshot
    }

    fn ledger_write_profile(&self) -> LiveLedgerWriteProfile {
        LiveLedgerWriteProfile::AtomicHeadEventsSourcesLifecycleAdmissionStageExecution
    }

    async fn read_live_history(
        &self,
        request: &LiveHistoryReadRequest,
    ) -> Result<LiveHistoryWindow, LiveHistoryReadError> {
        let inner = self.inner.lock().await;
        let sid = &request.head().session_id;
        validate_memory_live_prefix(&inner, sid)?;
        let current = inner
            .live_heads
            .get(sid)
            .ok_or(LiveHistoryReadError::SnapshotExpired)?;
        let records = inner.live_records.get(sid);
        let end = request.head().event_count;
        let last = if end == 0 {
            None
        } else {
            records
                .and_then(|rows| rows.get(&end))
                .map(|event| event.witness)
        };
        let next = if end < current.head.reference.event_count {
            records
                .and_then(|rows| rows.get(&(end + 1)))
                .map(|event| event.witness)
        } else {
            None
        };
        validate_retained_prefix(request.head(), &current.head.reference, last, next)?;
        let (rows, more) = memory_live_window(&inner, request.selection(), Some(request.head()))?;
        LiveHistoryWindow::new(request, rows, more)
    }

    async fn lookup_live_source(
        &self,
        source: &meerkat_core::live_execution::request::LiveSourceKey,
    ) -> Result<Option<LiveSourceRow>, RuntimeStoreError> {
        crate::live_ledger::source::validate_source_storage_key(source)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let inner = self.inner.lock().await;
        if memory_live_source(&inner, source).is_some()
            && !inner.live_heads.contains_key(source.session_id())
        {
            return Err(RuntimeStoreError::ReadFailed(
                "live source row has no committed head".into(),
            ));
        }
        if let Some(head) = inner.live_heads.get(source.session_id()) {
            head.head.validate_payload()?;
        }
        memory_live_source(&inner, source)
            .map(|row| {
                LiveSourceRow::restore(
                    source.clone(),
                    row.bytes().to_vec(),
                    row.digest().as_bytes(),
                )
            })
            .transpose()
    }

    async fn load_live_head(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<LiveLedgerStoredHead>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        validate_memory_live_prefix(&inner, session_id)?;
        if let Some(head) = inner.live_heads.get(session_id) {
            head.head.validate_payload()?;
        }
        let captured = inner
            .live_heads
            .get(session_id)
            .map(|stored| stored.head.clone());
        drop(inner);
        #[cfg(test)]
        let pause = self
            .live_head_load_after_capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        #[cfg(test)]
        if let Some((entered, release)) = pause {
            entered.notify_one();
            release.notified().await;
        }
        Ok(captured)
    }

    async fn commit_live_ledger(
        &self,
        prepared: PreparedLiveLedgerCommit,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<LiveLedgerCommitOutcome, RuntimeStoreError> {
        let encoded = prepared.encoded_records()?;
        let operation = prepared.operation_digest(&encoded)?;
        let mut inner = self.inner.lock().await;
        validate_memory_live_prefix(&inner, prepared.session_id())?;
        let stored = inner.live_heads.get(prepared.session_id());
        let before = stored.map(|stored| &stored.head);
        if let Some(head) = before {
            head.validate_payload()?;
        }
        let runtime_id = LogicalRuntimeId::for_session(prepared.session_id());
        for fence in prepared.input_read_fences() {
            enforce_memory_input_row_version(
                inner.input_states.get(&runtime_id.0),
                fence.input_id(),
                fence.expected_row_digest(),
            )?;
            enforce_memory_live_lifecycle_version(
                &inner,
                &runtime_id,
                prepared.expected_lifecycle(),
            )?;
        }
        if before == Some(prepared.successor()) {
            if stored.is_none_or(|stored| stored.operation != operation) {
                return Ok(LiveLedgerCommitOutcome::Conflict {
                    current: before.map(|head| head.reference.clone()),
                });
            }
            for source in prepared.sources() {
                if memory_live_source(&inner, source.replacement.source()).is_none_or(|row| {
                    row.bytes() != source.replacement.bytes()
                        || row.digest() != source.replacement.digest()
                }) {
                    return Err(RuntimeStoreError::ReadFailed(
                        "live source replay differs from committed content".into(),
                    ));
                }
            }
            for input in prepared
                .input_admission()
                .into_iter()
                .chain(prepared.input_stage().map(|stage| stage.input()))
            {
                let runtime_id = LogicalRuntimeId::for_session(prepared.session_id());
                let current = inner
                    .input_states
                    .get(&runtime_id.0)
                    .and_then(|states| states.get(&input.as_stored().state.input_id))
                    .ok_or_else(|| {
                        RuntimeStoreError::ReadFailed("Live admission input is missing".into())
                    })?;
                if serde_json::to_vec(current)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                    != serde_json::to_vec(input.as_stored())
                        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?
                {
                    return Err(RuntimeStoreError::ReadFailed(
                        "Live admission replay input differs".into(),
                    ));
                }
            }
            if let Some(stage) = prepared.input_stage() {
                let runtime_id = LogicalRuntimeId::for_session(prepared.session_id());
                let expected = stage.lifecycle().store_record().encode()?;
                if inner.runtime_lifecycle.get(&runtime_id.0) != Some(&expected) {
                    return Err(RuntimeStoreError::ReadFailed(
                        "Live stage replay lifecycle differs".into(),
                    ));
                }
            }
            for ((record, bytes), witness) in prepared
                .records()
                .iter()
                .zip(&encoded)
                .zip(prepared.prefix_witnesses(&encoded))
            {
                let stored = inner
                    .live_records
                    .get(prepared.session_id())
                    .and_then(|records| records.get(&record.sequence().get()))
                    .ok_or_else(|| {
                        RuntimeStoreError::ReadFailed("live replay record missing".into())
                    })?;
                if stored
                    .record
                    .encode()
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                    != *bytes
                    || stored.witness != witness
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
        if before.map(|head| &head.reference) != prepared.expected()
            || before.is_some_and(|head| head.reference == prepared.successor().reference)
        {
            return Ok(LiveLedgerCommitOutcome::Conflict {
                current: before.map(|head| head.reference.clone()),
            });
        }
        let actor = inner
            .session_authorities
            .get(&LogicalRuntimeId::for_session(prepared.session_id()).0)
            .cloned()
            .map(RuntimeSessionAuthority::WholeBlob);
        if (actor.is_none() && !prepared.is_archive_ingress_fence())
            || prepared
                .expected_actor()
                .is_some_and(|expected| Some(expected) != actor.as_ref())
        {
            return Ok(LiveLedgerCommitOutcome::ActorConflict { current: actor });
        }
        if prepared.input_read_fences().is_empty() {
            enforce_memory_live_lifecycle_version(
                &inner,
                &runtime_id,
                prepared.expected_lifecycle(),
            )?;
        }
        let mut source_charges = LiveSourceChargeDelta::default();
        for source in prepared.sources() {
            let current = memory_live_source(&inner, source.replacement.source());
            prepared.validate_new_source_context(source, current)?;
            match source.check(current)? {
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
        prepared.validate(before, &encoded, source_charges)?;
        let runtime_id = LogicalRuntimeId::for_session(prepared.session_id());
        let input_mutations = if let Some(input) = prepared.input_admission() {
            let bundle = input.as_stored();
            if inner
                .input_states
                .get(&runtime_id.0)
                .is_some_and(|states| states.contains_key(&bundle.state.input_id))
            {
                return Err(RuntimeStoreError::InputRowVersionConflict {
                    input_id: bundle.state.input_id.to_string(),
                });
            }
            Some(prepare_memory_input_state_mutations(
                &inner,
                &runtime_id.0,
                vec![MemoryInputStateMutation::Upsert(input.clone_stored())],
            )?)
        } else if let Some(stage) = prepared.input_stage() {
            let input = stage.input();
            precheck_fenced_input_updates(
                inner.input_states.get(&runtime_id.0),
                &[(
                    input.clone_stored(),
                    input.expected_row_digest().map(str::to_owned),
                )],
            )?;
            Some(prepare_memory_input_state_mutations(
                &inner,
                &runtime_id.0,
                vec![MemoryInputStateMutation::Upsert(input.clone_stored())],
            )?)
        } else {
            None
        };
        let staged_lifecycle = prepared
            .input_stage()
            .map(|stage| {
                stage
                    .lifecycle()
                    .store_record()
                    .encode()
                    .map(|bytes| (bytes, stage.lifecycle().runtime_state()))
            })
            .transpose()?;
        // Every fallible check is above this publication: Memory has no rollback.
        super::super::execute_optional_runtime_store_target_write(
            Some(write_fence.as_ref()),
            || {
                if let Some((bytes, state)) = staged_lifecycle {
                    inner.runtime_lifecycle.insert(runtime_id.0.clone(), bytes);
                    sync_runtime_session_catalog_lifecycle(&mut inner, &runtime_id.0, state);
                }
                if let Some(mutations) = input_mutations {
                    apply_prepared_memory_input_state_mutations(
                        &mut inner,
                        &runtime_id.0,
                        mutations,
                    );
                }
                let rows = inner
                    .live_records
                    .entry(prepared.session_id().clone())
                    .or_default();
                for (record, witness) in prepared
                    .records()
                    .iter()
                    .zip(prepared.prefix_witnesses(&encoded))
                {
                    rows.insert(
                        record.sequence().get(),
                        StoredLiveLedgerEvent {
                            record: record.clone(),
                            witness,
                        },
                    );
                }
                for source in prepared.sources() {
                    inner
                        .live_sources
                        .entry(prepared.session_id().clone())
                        .or_default()
                        .insert(
                            source.replacement.source().clone(),
                            source.replacement.clone(),
                        );
                }
                inner.live_heads.insert(
                    prepared.session_id().clone(),
                    StoredLiveLedgerCommit {
                        head: prepared.successor().clone(),
                        operation,
                    },
                );
                Ok(())
            },
        )?;
        Ok(LiveLedgerCommitOutcome::Committed {
            head: prepared.successor().reference.clone(),
        })
    }

    async fn capture_live_composite(
        &self,
        request: &LiveCompositeReadRequest,
    ) -> Result<Option<LiveCompositeCapture>, RuntimeStoreError> {
        let runtime_id = LogicalRuntimeId::for_session(request.session_id());
        let captured = {
            let inner = self.inner.lock().await;
            validate_memory_live_prefix(&inner, request.session_id())?;
            let head = inner
                .live_heads
                .get(request.session_id())
                .map(|head| head.head.reference.clone());
            let records = inner.live_records.get(request.session_id());
            if head.is_none() && records.is_some_and(|records| !records.is_empty()) {
                return Err(RuntimeStoreError::ReadFailed(
                    "live records exist without a committed head".into(),
                ));
            }
            let (bytes, actor) = match (
                inner.sessions.get(&runtime_id.0),
                inner.session_authorities.get(&runtime_id.0),
            ) {
                (None, None) if head.is_none() => return Ok(None),
                (Some(bytes), Some(actor)) => (Arc::clone(bytes), actor.clone()),
                _ => {
                    return Err(RuntimeStoreError::ReadFailed(
                        "live composite actor and content authority disagree".into(),
                    ));
                }
            };
            let end = head.as_ref().map_or(0, |head| head.event_count);
            if request.after_sequence() > end {
                return Err(RuntimeStoreError::ReadFailed(
                    "live composite window exceeds its captured head".into(),
                ));
            }
            let (records, more) = memory_live_window(&inner, request, head.as_ref())?;
            (bytes, actor, head, records, more)
        };
        LiveCompositeCapture::whole_blob(captured.0, captured.1, captured.2, captured.3, captured.4)
            .map(Some)
    }
}
