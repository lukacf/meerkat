use super::*;
use crate::live_ledger::history::{LiveObservationHistoryReader, RuntimeLiveObservationReader};
use crate::live_ledger::write::append_observation_fixture;
use crate::store::{RuntimeStore, SerializedSessionSnapshot};
use meerkat_contracts::wire::live_observation::{
    LIVE_OBSERVATION_REPLY_MAX_BYTES, LiveObservationOwner, LiveObservationPage,
    LiveObservationPageQuery, LiveObservationReadFailure, LiveObservationWireCodecV1 as Codec,
};
use meerkat_contracts::wire::supervisor_bridge::BridgeMemberLiveObservationPageRequest;
use std::sync::atomic::{AtomicUsize, Ordering};

struct RetainedHistoryHost {
    reader: RuntimeLiveObservationReader,
    owner: LiveObservationOwner,
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    release: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait::async_trait]
impl MemberObservationHost for RetainedHistoryHost {
    async fn member_generation(&self, _: &SessionId) -> Result<u64, MemberObservationError> {
        panic!("Live history must not read ordinary generation history")
    }

    async fn read_history(
        &self,
        _: &SessionId,
        _: Option<u64>,
        _: Option<u32>,
    ) -> Result<MemberHistoryWindow, MemberObservationError> {
        panic!("Live history must not fall back to ordinary Messages")
    }

    async fn poll_events(
        &self,
        _: &SessionId,
        _: MemberEventsPollRequest<'_>,
    ) -> Result<MemberEventsWindow, MemberObservationError> {
        panic!("Live history must not use the event projection")
    }

    async fn read_live_observations(
        &self,
        session: &SessionId,
        query: LiveObservationPageQuery,
    ) -> Result<LiveObservationPage, MemberObservationError> {
        assert_eq!(session, self.owner.session_id());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if let Some(release) = &self.release {
            release.notified().await;
        }
        self.reader
            .read(self.owner.clone(), query)
            .await
            .map_err(|error| MemberObservationError::LiveHistory {
                failure: error.failure(),
                reason: error.to_string(),
            })
    }

    async fn open_directed_turn_window(
        &self,
        _: &SessionId,
        _: &BridgeMemberIncarnation,
        _: &str,
        _: Option<&meerkat_contracts::wire::supervisor_bridge::BridgeBoundedResultSpec>,
    ) -> Result<
        crate::member_observation::DirectedTurnWindow,
        crate::member_observation::DirectedTurnReject,
    > {
        panic!("history must not open directed work")
    }

    async fn cancel_directed_turn_window(
        &self,
        _: &SessionId,
        _: crate::member_observation::DirectedTurnWindow,
    ) -> Result<(), crate::member_observation::DirectedTurnReject> {
        panic!("history must not cancel directed work")
    }

    async fn admit_directed_turn(
        &self,
        _: &SessionId,
        _: DirectedTurnAdmission,
    ) -> Result<(), crate::member_observation::DirectedTurnReject> {
        panic!("history must not admit directed work")
    }
}

fn install_reader(
    fixture: &LiveArmFixture,
    store: Arc<dyn RuntimeStore>,
    release: Option<Arc<tokio::sync::Notify>>,
) -> Arc<RetainedHistoryHost> {
    let host = Arc::new(RetainedHistoryHost {
        reader: RuntimeLiveObservationReader::new(store),
        owner: LiveObservationOwner::Member {
            session_id: fixture.session_id.clone(),
            mob_id: fixture.incarnation.mob_id.clone(),
            agent_identity: fixture.incarnation.agent_identity.clone(),
        },
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release,
    });
    fixture.adapter.set_member_observation_host(host.clone());
    host
}

fn request(fixture: &LiveArmFixture, query: LiveObservationPageQuery) -> BridgeCommand {
    BridgeCommand::ReadMemberLiveObservations(BridgeMemberLiveObservationPageRequest {
        supervisor: fixture.supervisor_spec.clone().into(),
        epoch: 1,
        protocol_version: BridgeProtocolVersion::V7,
        expected_member: fixture.incarnation.clone(),
        query,
    })
}

async fn seed(store: &dyn RuntimeStore, session: &SessionId, text: &str) {
    store
        .commit_session_snapshot(
            &crate::identifiers::LogicalRuntimeId::for_session(session),
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(
                    serde_json::to_vec(&meerkat_core::Session::with_id(session.clone()))
                        .expect("session"),
                ),
            },
        )
        .await
        .expect("persist session");
    append_observation_fixture(store, session, &[text, text, text, text])
        .await
        .expect("real ledger CAS");
}

#[tokio::test]
async fn public_live_history_v7_dispatch_legacy_default_and_scope_refusals() {
    let fixture = LiveArmFixture::bound("history-protocol").await;
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    seed(store.as_ref(), &fixture.session_id, "retained").await;
    let host = install_reader(&fixture, store, None);
    assert_eq!(BridgeProtocolVersion::default(), BridgeProtocolVersion::V6);
    let mut command = request(&fixture, LiveObservationPageQuery::default());
    let BridgeCommand::ReadMemberLiveObservations(payload) = &mut command else {
        unreachable!()
    };
    payload.protocol_version = BridgeProtocolVersion::V6;
    fixture.serve(&command).await;
    assert!(matches!(
        fixture.next_reply("V6 page refusal").await,
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::UnsupportedProtocolVersion,
            ..
        }
    ));
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);

    let mut command = request(&fixture, LiveObservationPageQuery::default());
    let BridgeCommand::ReadMemberLiveObservations(payload) = &mut command else {
        unreachable!()
    };
    payload.epoch = 0;
    fixture.serve(&command).await;
    assert!(matches!(
        fixture.next_reply("unauthorized page").await,
        BridgeReply::Rejected { .. }
    ));
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);

    fixture
        .serve(&request(&fixture, LiveObservationPageQuery::default()))
        .await;
    assert!(matches!(
        fixture.next_reply("V7 page").await,
        BridgeReply::MemberLiveObservationPage(_)
    ));
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);

    let live = Arc::new(ScriptedMemberLive::default());
    fixture.adapter.set_member_live_host(live.clone());
    let mut open = BridgeLiveOpenPayload {
        supervisor: fixture.supervisor_spec.clone().into(),
        epoch: 1,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member: fixture.incarnation.clone(),
        profile: None,
        turning_mode: None,
        transport: None,
    };
    fixture
        .serve(&BridgeCommand::OpenMemberLiveChannel(open.clone()))
        .await;
    assert!(matches!(
        fixture.next_reply("legacy V4 open").await,
        BridgeReply::MemberLiveChannelOpened(_)
    ));
    let calls = live.recorded().len();
    open.profile = Some(
        meerkat_contracts::wire::supervisor_bridge::BridgeLiveProfileSelection::V1 {
            profile_id: meerkat_core::live_execution::profile::LiveProfileId::parse("voice")
                .expect("profile"),
        },
    );
    fixture
        .serve(&BridgeCommand::OpenMemberLiveChannel(open))
        .await;
    assert!(matches!(
        fixture.next_reply("V4 profile refusal").await,
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::UnsupportedProtocolVersion,
            ..
        }
    ));
    assert_eq!(live.recorded().len(), calls);
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn public_live_history_actual_tcp_reply_retains_prefix_after_append_and_reopen() {
    let fixture = LiveArmFixture::bound("history-reopen").await;
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("history.sqlite3");
    let store: Arc<dyn RuntimeStore> =
        Arc::new(crate::store::SqliteRuntimeStore::new_whole_blob(&path).expect("store"));
    let text = "\0\\\"\n".repeat(4000);
    seed(store.as_ref(), &fixture.session_id, &text).await;
    let owner = install_reader(&fixture, store.clone(), None).owner.clone();
    let mut query = LiveObservationPageQuery::default();
    fixture.serve(&request(&fixture, query.clone())).await;
    let BridgeReply::MemberLiveObservationPage(first) = fixture.next_reply("first real page").await
    else {
        panic!("page")
    };
    Codec::validate_page_response(&first, &owner, &query).expect("exact response");
    assert!(
        Codec::encode_reply(&first).expect("full reply").len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES
    );
    assert!(first.has_more);
    assert!(!first.records.is_empty());
    let snapshot = first.snapshot.clone();
    let mut sequences: Vec<_> = first
        .records
        .iter()
        .map(|record| record.sequence.get())
        .collect();
    append_observation_fixture(
        store.as_ref(),
        &fixture.session_id,
        &["later outside prefix"],
    )
    .await
    .expect("append");
    let reopened: Arc<dyn RuntimeStore> =
        Arc::new(crate::store::SqliteRuntimeStore::new_whole_blob(&path).expect("reopen"));
    install_reader(&fixture, reopened, None);
    drop(store);
    let mut next = first.next_cursor;
    while let Some(cursor) = next {
        query = LiveObservationPageQuery::new(None, Some(cursor), 64).expect("resume");
        fixture.serve(&request(&fixture, query.clone())).await;
        let BridgeReply::MemberLiveObservationPage(page) =
            fixture.next_reply("resumed real page").await
        else {
            panic!("page")
        };
        Codec::validate_page_response(&page, &owner, &query).expect("response");
        assert_eq!(page.snapshot, snapshot);
        assert!(
            Codec::encode_reply(&page).expect("full reply").len()
                <= LIVE_OBSERVATION_REPLY_MAX_BYTES
        );
        assert!(!page.records.is_empty());
        sequences.extend(page.records.iter().map(|record| record.sequence.get()));
        assert!(
            sequences.len() <= 4,
            "cursor loop must make bounded nonempty progress"
        );
        next = page.next_cursor;
    }
    assert_eq!(sequences, vec![1, 2, 3, 4]);
    assert!(
        fixture.adapter.member_live_host().is_none(),
        "retained reads never install an active channel"
    );
}

#[tokio::test]
async fn public_live_history_rechecks_member_after_storage_await() {
    let fixture = LiveArmFixture::bound("history-stale").await;
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    seed(store.as_ref(), &fixture.session_id, "retained").await;
    let release = Arc::new(tokio::sync::Notify::new());
    let host = install_reader(&fixture, store, Some(release.clone()));
    let command = request(&fixture, LiveObservationPageQuery::default());
    let serving = fixture.serve(&command);
    let rotate = async {
        host.entered.notified().await;
        let mut successor = fixture.incarnation.clone();
        successor.fence_token += 1;
        register_member_incarnation(&fixture.adapter, fixture.session_id.clone(), successor)
            .await
            .expect("rotate");
        release.notify_one();
    };
    tokio::join!(serving, rotate);
    assert!(matches!(
        fixture.next_reply("stale completion").await,
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::StaleFence,
            ..
        }
    ));
}

#[tokio::test]
async fn public_live_history_absent_capability_does_not_fall_back() {
    let fixture = LiveArmFixture::bound("history-unsupported").await;
    fixture
        .adapter
        .set_member_observation_host(Arc::new(ExhaustedDirectedTurnObservation));
    fixture
        .serve(&request(&fixture, LiveObservationPageQuery::default()))
        .await;
    assert!(matches!(
        fixture.next_reply("unsupported host").await,
        BridgeReply::Rejected {
            cause: BridgeRejectionCause::LiveObservationRead {
                failure: LiveObservationReadFailure::Unsupported
            },
            ..
        }
    ));
}
