#![cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]

use std::path::Path;

use meerkat_contracts::wire::live_observation::{
    LIVE_OBSERVATION_REPLY_MAX_BYTES, LIVE_OBSERVATION_TEXT_MAX_BYTES, LiveObservationCoverage,
    LiveObservationCursor, LiveObservationEncodingError, LiveObservationFilter,
    LiveObservationOwner, LiveObservationRecord, LiveObservationWireCodecV1 as Codec,
};
use meerkat_contracts::wire::supervisor_bridge::BridgeReply;
use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{
    LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use meerkat_runtime::live_ledger::completion::{
    LiveChannelControlOutcome, LiveCompletionEvent, LiveCompletionRecord, LiveCompletionText,
};
use meerkat_runtime::live_ledger::history::{
    LiveObservationHistoryError, LiveObservationHistoryQuery, read_observation_history,
    read_observation_page,
};
use meerkat_runtime::live_ledger::record::LiveLedgerRecord;
use meerkat_runtime::live_ledger::transcript::{
    KnownLiveReceiveGap, LiveDiscontinuity, LiveHeadReference, LiveLedgerFormatV1,
    LiveLedgerPrefixDigest, StoredLiveObservation,
};
use meerkat_runtime::live_ledger::write::LIVE_HEAD_STORAGE_ALLOWANCE_BYTES;
use meerkat_runtime::live_resources::{LIVE_EVENT_STORAGE_ALLOWANCE_BYTES, LiveResourceCharge};
use meerkat_runtime::store::SqliteRuntimeStore;
use meerkat_runtime::store::live_history::LiveHistoryReadError;
use sha2::{Digest, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn observation(sequence: u64, channel: &str, text: String) -> TestResult<LiveLedgerRecord> {
    let fit = Codec::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(sequence)?,
        channel_id: LiveChannelId::new(channel),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(1066.5390310178611, 1066.5390310178614)?,
            text,
        ),
    })?;
    Ok(LiveLedgerRecord::Observation(
        StoredLiveObservation::from_fit(&fit),
    ))
}

fn control(session: &SessionId, sequence: u64, channel: &str) -> TestResult<LiveLedgerRecord> {
    Ok(LiveLedgerRecord::Completion(LiveCompletionRecord {
        format: LiveLedgerFormatV1::V1,
        session_id: session.clone(),
        channel_id: LiveChannelId::new(channel),
        sequence: LiveObservationSeq::new(sequence)?,
        event: LiveCompletionEvent::ChannelControl {
            outcome: LiveChannelControlOutcome::RecoveryFenced,
            diagnostic: LiveCompletionText::new("\0".repeat(1024))?,
        },
    }))
}

// Physical persisted-image fixture only; production prepared commits cannot
// be minted by this external test. Writer tests separately exercise real CAS.
fn append_image(
    path: &Path,
    before: &LiveHeadReference,
    records: &[LiveLedgerRecord],
) -> TestResult<LiveHeadReference> {
    let mut head = before.clone();
    head.revision += 1;
    let mut conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO runtime_live_heads
         (session_id,format_version,generation,revision,event_count,prefix_digest,commit_digest,
          used_records,used_bytes,reserved_records,reserved_bytes,ingress_generation,
          transcript_snapshot,request_snapshot)
         VALUES (?1,1,?2,?3,0,?4,zeroblob(32),0,?5,0,0,1,x'',x'')",
        rusqlite::params![
            head.session_id.to_string(),
            head.generation,
            head.revision,
            head.prefix_digest.as_bytes().as_slice(),
            LIVE_HEAD_STORAGE_ALLOWANCE_BYTES,
        ],
    )?;
    let mut charge = LiveResourceCharge::default();
    for record in records {
        head.event_count += 1;
        assert_eq!(record.sequence().get(), head.event_count);
        let bytes = record.encode()?;
        let unit = LiveResourceCharge::for_event_record(&bytes)?;
        assert_eq!(
            unit.encoded_bytes,
            bytes.len() as u64 + LIVE_EVENT_STORAGE_ALLOWANCE_BYTES
        );
        charge = charge.checked_add(unit)?;
        head.prefix_digest = head.prefix_digest.appended(record.sequence(), &bytes);
        tx.execute(
            "INSERT INTO runtime_live_events
             (session_id,sequence,channel_id,record,record_digest,commit_revision,prefix_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                head.session_id.to_string(),
                record.sequence().get(),
                record.channel_id().as_str(),
                bytes,
                Sha256::digest(&bytes).as_slice(),
                head.revision,
                head.prefix_digest.as_bytes().as_slice(),
            ],
        )?;
    }
    tx.execute(
        "UPDATE runtime_live_heads SET revision=?2,event_count=?3,prefix_digest=?4,
         used_records=used_records+?5,used_bytes=used_bytes+?6 WHERE session_id=?1",
        rusqlite::params![
            head.session_id.to_string(),
            head.revision,
            head.event_count,
            head.prefix_digest.as_bytes().as_slice(),
            charge.records,
            charge.encoded_bytes,
        ],
    )?;
    tx.commit()?;
    Ok(head)
}

fn empty_head() -> LiveHeadReference {
    let session_id = SessionId::new();
    // SQLite's maximum persisted integer; wire u64 extrema are tested in contracts.
    let generation = i64::MAX as u64;
    LiveHeadReference {
        format: LiveLedgerFormatV1::V1,
        prefix_digest: LiveLedgerPrefixDigest::empty(&session_id, generation),
        session_id,
        generation,
        revision: i64::MAX as u64 - 10,
        event_count: 0,
    }
}

fn query(
    head: &LiveHeadReference,
    filter: LiveObservationFilter,
    cursor: Option<LiveObservationCursor>,
    limit: usize,
) -> LiveObservationHistoryQuery {
    LiveObservationHistoryQuery {
        owner: LiveObservationOwner::Member {
            session_id: head.session_id.clone(),
            mob_id: "\0".repeat(128),
            agent_identity: "\0".repeat(128),
        },
        head: head.clone(),
        coverage: LiveObservationCoverage::UnknownExtentCrashDiscontinuity,
        filter,
        cursor,
        limit,
    }
}

fn reopen(path: &Path, head_canonical: bool) -> TestResult<SqliteRuntimeStore> {
    Ok(if head_canonical {
        SqliteRuntimeStore::new_head_canonical(path)?
    } else {
        SqliteRuntimeStore::new_whole_blob(path)?
    })
}

#[tokio::test]
async fn escaped_accepted_prefix_pages_survive_head_advance_reopen_and_filtered_controls()
-> TestResult {
    for head_canonical in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        drop(reopen(&path, head_canonical)?);
        let empty = empty_head();
        let channel = "\0".repeat(128);
        let mut records = Vec::new();
        for sequence in 1..=280 {
            records.push(if [1, 130, 270].contains(&sequence) {
                observation(
                    sequence,
                    &channel,
                    format!(" {sequence}\n\\{}", "\0".repeat(18_000)),
                )?
            } else if sequence == 140 {
                observation(sequence, "other", "other channel".into())?
            } else {
                control(&empty.session_id, sequence, &channel)?
            });
        }
        let first = append_image(&path, &empty, &records)?;
        let captured = append_image(&path, &first, &[])?;
        append_image(
            &path,
            &captured,
            &[observation(281, &channel, "later excluded".into())?],
        )?;
        for filter in [
            LiveObservationFilter::AllChannels {},
            LiveObservationFilter::Channel {
                channel_id: LiveChannelId::new(&channel),
            },
            LiveObservationFilter::Channel {
                channel_id: LiveChannelId::new("absent"),
            },
        ] {
            let expected = records.iter().filter_map(|record| match record {
                LiveLedgerRecord::Observation(stored)
                    if matches!(&filter, LiveObservationFilter::AllChannels {})
                        || matches!(&filter, LiveObservationFilter::Channel { channel_id } if channel_id == &stored.record().channel_id) =>
                {
                    Some(stored.record().clone())
                }
                _ => None,
            }).collect::<Vec<_>>();
            for limit in [1, 256] {
                let mut cursor = None;
                let mut observed = Vec::new();
                let mut previous = 0;
                for page_number in 0..=expected.len() {
                    let store = reopen(&path, head_canonical)?;
                    let request = query(&captured, filter.clone(), cursor, limit);
                    let page = read_observation_page(&store, request).await?;
                    assert_eq!(page.snapshot.revision, captured.revision);
                    assert_eq!(page.snapshot.end_sequence, captured.event_count);
                    assert_eq!(
                        page.snapshot.prefix_digest,
                        captured.prefix_digest.to_string()
                    );
                    assert_eq!(page.after_sequence, previous);
                    let bytes = Codec::encode_reply(&page)?;
                    assert!(bytes.len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES);
                    assert_eq!(
                        bytes,
                        serde_json::to_vec(&BridgeReply::MemberLiveObservationPage(page.clone()))?
                    );
                    assert_eq!(
                        serde_json::from_slice::<BridgeReply>(&bytes)?,
                        BridgeReply::MemberLiveObservationPage(page.clone())
                    );
                    observed.extend(page.records.iter().map(|record| record.as_ref().clone()));
                    if !page.has_more {
                        assert!(page.next_cursor.is_none());
                        break;
                    }
                    assert!(page_number < expected.len(), "pagination must terminate");
                    let last = page
                        .records
                        .last()
                        .ok_or("empty continuation page")?
                        .sequence
                        .get();
                    assert!(last > previous);
                    cursor = page.next_cursor;
                    previous = Codec::cursor_after_sequence(
                        cursor.as_ref().ok_or("cursor")?,
                        &page.owner,
                        &page.filter,
                        &page.snapshot,
                    )?;
                    assert_eq!(previous, last);
                }
                assert_eq!(observed, expected);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn maximal_admitted_rows_fit_actual_reopened_reply_and_oversize_never_enters_image()
-> TestResult {
    for unit in ["\n", "\\", "\0"] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        drop(reopen(&path, false)?);
        let empty = empty_head();
        let channel = "\0".repeat(128);
        let mut low = 0;
        let mut high = LIVE_OBSERVATION_TEXT_MAX_BYTES;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            match observation(1, &channel, unit.repeat(mid)) {
                Ok(_) => low = mid,
                Err(error)
                    if matches!(
                        error.downcast_ref::<LiveObservationEncodingError>(),
                        Some(LiveObservationEncodingError::EncodedReplyTooLarge)
                    ) =>
                {
                    high = mid - 1;
                }
                Err(error) => return Err(error),
            }
        }
        assert!(low < LIVE_OBSERVATION_TEXT_MAX_BYTES);
        assert!(observation(1, &channel, unit.repeat(low + 1)).is_err());
        let accepted = observation(1, &channel, unit.repeat(low))?;
        let head = append_image(
            &path,
            &empty,
            &[
                accepted.clone(),
                observation(2, &channel, unit.repeat(low))?,
            ],
        )?;
        let store = reopen(&path, false)?;
        let page = read_observation_page(
            &store,
            query(
                &head,
                LiveObservationFilter::Channel {
                    channel_id: LiveChannelId::new(&channel),
                },
                None,
                256,
            ),
        )
        .await?;
        let LiveLedgerRecord::Observation(stored) = accepted else {
            return Err("observation".into());
        };
        assert_eq!(
            page.records.first().ok_or("record")?.as_ref(),
            stored.record()
        );
        assert!(
            Codec::encode_reply(&page)?.len() <= stored.wire_receipt().maximal_single_reply_bytes
        );
        assert!(page.has_more);
        let next =
            read_observation_page(&store, query(&head, page.filter, page.next_cursor, 256)).await?;
        assert_eq!(next.records.len(), 1);
        assert_eq!(next.records[0].sequence.get(), 2);
        assert!(!next.has_more);
    }
    Ok(())
}

#[tokio::test]
async fn malformed_mismatched_and_expired_queries_fail_without_fallback_to_current_head()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    drop(reopen(&path, false)?);
    let empty = empty_head();
    let head = append_image(
        &path,
        &empty,
        &[
            observation(1, "voice", "one".into())?,
            observation(2, "voice", "two".into())?,
        ],
    )?;
    let store = reopen(&path, false)?;
    let filter = LiveObservationFilter::AllChannels {};
    let first = read_observation_page(&store, query(&head, filter.clone(), None, 1)).await?;
    let cursor = first.next_cursor.ok_or("cursor")?;
    let changed = append_image(&path, &head, &[])?;
    assert!(matches!(
        read_observation_page(
            &store,
            query(&changed, filter.clone(), Some(cursor.clone()), 1)
        )
        .await,
        Err(LiveObservationHistoryError::Encoding(
            LiveObservationEncodingError::CursorMismatch
        ))
    ));
    assert!(matches!(
        read_observation_page(
            &store,
            query(
                &head,
                LiveObservationFilter::Channel {
                    channel_id: LiveChannelId::new("voice")
                },
                Some(cursor),
                1
            )
        )
        .await,
        Err(LiveObservationHistoryError::Encoding(
            LiveObservationEncodingError::CursorMismatch
        ))
    ));
    let malformed = serde_json::from_str::<LiveObservationCursor>("\"not a cursor\"")?;
    assert!(matches!(
        read_observation_page(&store, query(&head, filter.clone(), Some(malformed), 1)).await,
        Err(LiveObservationHistoryError::Encoding(
            LiveObservationEncodingError::InvalidCursor
        ))
    ));
    let mut expired = head.clone();
    expired.generation -= 1;
    assert!(matches!(
        read_observation_page(&store, query(&expired, filter.clone(), None, 1)).await,
        Err(LiveObservationHistoryError::Read(
            LiveHistoryReadError::SnapshotExpired
        ))
    ));
    let mut invented = head.clone();
    invented.prefix_digest = LiveLedgerPrefixDigest::from_sha256([0; 32]);
    assert!(matches!(
        read_observation_page(&store, query(&invented, filter, None, 1)).await,
        Err(LiveObservationHistoryError::Read(
            LiveHistoryReadError::InvalidSnapshot
        ))
    ));
    Ok(())
}

fn discontinuity(
    head: &LiveHeadReference,
    sequence: u64,
    channel: &str,
    unknown: bool,
) -> TestResult<LiveLedgerRecord> {
    Ok(LiveLedgerRecord::Completion(LiveCompletionRecord {
        format: LiveLedgerFormatV1::V1,
        session_id: head.session_id.clone(),
        channel_id: LiveChannelId::new(channel),
        sequence: LiveObservationSeq::new(sequence)?,
        event: LiveCompletionEvent::ChannelDiscontinuity {
            discontinuity: if unknown {
                LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
                    last_accepted_head: head.clone(),
                    old_incarnation: LiveChannelId::new(channel),
                }
            } else {
                LiveDiscontinuity::KnownLocalGap {
                    channel_id: LiveChannelId::new(channel),
                    observed_bounds: KnownLiveReceiveGap::new(2, 4)?,
                }
            },
        },
    }))
}

#[tokio::test]
async fn cursor_only_resume_verifies_retained_coverage_without_an_active_channel() -> TestResult {
    for head_canonical in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        drop(reopen(&path, head_canonical)?);
        let empty = empty_head();
        let prefix = append_image(
            &path,
            &empty,
            &[
                observation(1, "voice", "one".into())?,
                observation(2, "other", "other".into())?,
                observation(3, "voice", "three".into())?,
            ],
        )?;
        let captured = append_image(
            &path,
            &prefix,
            &[
                discontinuity(&prefix, 4, "voice", false)?,
                discontinuity(&prefix, 5, "other", true)?,
            ],
        )?;
        let owner = query(&captured, LiveObservationFilter::AllChannels {}, None, 1).owner;
        let filters = [
            (
                LiveObservationFilter::AllChannels {},
                LiveObservationCoverage::UnknownExtentCrashDiscontinuity,
                vec![1, 2, 3],
            ),
            (
                LiveObservationFilter::Channel {
                    channel_id: LiveChannelId::new("voice"),
                },
                LiveObservationCoverage::KnownLocalGap,
                vec![1, 3],
            ),
            (
                LiveObservationFilter::Channel {
                    channel_id: LiveChannelId::new("other"),
                },
                LiveObservationCoverage::UnknownExtentCrashDiscontinuity,
                vec![2],
            ),
            (
                LiveObservationFilter::Channel {
                    channel_id: LiveChannelId::new("absent"),
                },
                LiveObservationCoverage::CompleteAcceptedPrefix,
                vec![],
            ),
        ];
        let mut first_pages = Vec::new();
        for (filter, coverage, expected) in filters {
            let store = reopen(&path, head_canonical)?;
            let first = read_observation_history(&store, owner.clone(), filter, None, 1).await?;
            assert_eq!(first.snapshot.coverage, coverage);
            assert_eq!(first.snapshot.end_sequence, captured.event_count);
            first_pages.push((first, expected));
        }
        let later = append_image(
            &path,
            &captured,
            &[
                observation(6, "voice", "later".into())?,
                discontinuity(&captured, 7, "voice", true)?,
            ],
        )?;
        append_image(&path, &later, &[])?;
        for (mut page, expected) in first_pages {
            let snapshot = page.snapshot.clone();
            let mut sequences = Vec::new();
            for index in 0..=expected.len() {
                assert_eq!(page.snapshot, snapshot);
                assert!(Codec::encode_reply(&page)?.len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES);
                sequences.extend(page.records.iter().map(|record| record.sequence.get()));
                if !page.has_more {
                    break;
                }
                assert!(index < expected.len());
                assert!(!page.records.is_empty());
                let store = reopen(&path, head_canonical)?;
                page = read_observation_history(
                    &store,
                    owner.clone(),
                    page.filter,
                    page.next_cursor,
                    1,
                )
                .await?;
            }
            assert_eq!(sequences, expected);
        }
        let store = reopen(&path, head_canonical)?;
        let fresh = read_observation_history(
            &store,
            owner,
            LiveObservationFilter::Channel {
                channel_id: LiveChannelId::new("voice"),
            },
            None,
            256,
        )
        .await?;
        assert_eq!(
            fresh.snapshot.coverage,
            LiveObservationCoverage::UnknownExtentCrashDiscontinuity
        );
        assert_eq!(
            fresh
                .records
                .iter()
                .map(|record| record.sequence.get())
                .collect::<Vec<_>>(),
            [1, 3, 6]
        );
    }
    Ok(())
}

#[tokio::test]
async fn cursor_metadata_is_comparison_material_not_coverage_or_snapshot_authority() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    drop(reopen(&path, false)?);
    let empty = empty_head();
    let head = append_image(
        &path,
        &empty,
        &[
            observation(1, "voice", "one".into())?,
            observation(2, "voice", "two".into())?,
        ],
    )?;
    let store = reopen(&path, false)?;
    let owner = LiveObservationOwner::Session {
        session_id: head.session_id.clone(),
    };
    let filter = LiveObservationFilter::AllChannels {};
    let first = read_observation_history(&store, owner.clone(), filter.clone(), None, 1).await?;
    let fit = Codec::check_record_fit(
        first
            .records
            .first()
            .ok_or("first record")?
            .as_ref()
            .clone(),
    )?;
    let cursor = first.next_cursor.ok_or("cursor")?;
    for field in ["coverage", "prefix_digest", "generation", "revision"] {
        let mut changed = first.snapshot.clone();
        match field {
            "coverage" => changed.coverage = LiveObservationCoverage::KnownLocalGap,
            "prefix_digest" => changed.prefix_digest = format!("sha256:{}", "00".repeat(32)),
            "generation" => changed.generation = head.generation - 1,
            "revision" => changed.revision = head.revision + 1,
            _ => return Err("unexpected field".into()),
        }
        // Anyone can encode query material; the public codec has no authority.
        let cursor = Codec::page(
            owner.clone(),
            filter.clone(),
            changed,
            0,
            std::slice::from_ref(&fit),
            1,
            true,
        )?
        .next_cursor
        .ok_or("forged cursor")?;
        let result =
            read_observation_history(&store, owner.clone(), filter.clone(), Some(cursor), 1).await;
        match field {
            "coverage" => assert!(matches!(
                result,
                Err(LiveObservationHistoryError::Encoding(
                    LiveObservationEncodingError::CursorMismatch
                ))
            )),
            "generation" => assert!(matches!(
                result,
                Err(LiveObservationHistoryError::Read(
                    LiveHistoryReadError::SnapshotExpired
                ))
            )),
            _ => assert!(matches!(
                result,
                Err(LiveObservationHistoryError::Read(
                    LiveHistoryReadError::InvalidSnapshot
                ))
            )),
        }
    }
    assert!(matches!(
        read_observation_history(
            &store,
            LiveObservationOwner::Session {
                session_id: SessionId::new()
            },
            filter.clone(),
            Some(cursor),
            1,
        )
        .await,
        Err(LiveObservationHistoryError::Encoding(
            LiveObservationEncodingError::CursorMismatch
        ))
    ));
    assert!(matches!(
        read_observation_history(
            &store,
            LiveObservationOwner::Session {
                session_id: SessionId::new()
            },
            filter.clone(),
            None,
            1,
        )
        .await,
        Err(LiveObservationHistoryError::NoRetainedHistory)
    ));
    for limit in [0, 257] {
        assert!(matches!(
            read_observation_history(&store, owner.clone(), filter.clone(), None, limit).await,
            Err(LiveObservationHistoryError::Encoding(
                LiveObservationEncodingError::InvalidPageLimit
            ))
        ));
    }
    assert_eq!(
        head.prefix_digest
            .to_string()
            .parse::<LiveLedgerPrefixDigest>()?,
        head.prefix_digest,
    );
    for invalid in ["sha256:00", "sha256:é", "SHA256:00"] {
        assert!(invalid.parse::<LiveLedgerPrefixDigest>().is_err());
    }
    Ok(())
}
