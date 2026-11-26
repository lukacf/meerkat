//! Multi-host mobs phase 3 — controlling cold-restart with placed members
//! (design-revival R T-15/T-16; §19.L5 gotcha 12 + ADJ-8 fence reseed).
//!
//! The synthetic "placement present but owner_bridge_session_id=None ⇒ typed
//! error, not a silent skip" leg (W-F.2) is an IN-CRATE row (A2 lane): it
//! requires seeding corrupted machine state below the public builder surface.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

#[cfg(feature = "test-support")]
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_core::HandlingMode;
use meerkat_mob::AgentIdentity;
#[cfg(feature = "test-support")]
use meerkat_mob::MobEventKind;
use meerkat_mob::MobSessionService as _;
#[cfg(feature = "test-support")]
use meerkat_mob::event::{
    PlacedCompletionClosureEvent, PlacedCompletionHostOutcomeEvent, PlacedCompletionObligationEvent,
};
#[cfg(feature = "test-support")]
use meerkat_mob::runtime::host_actor::RecordedTrackedInputCancelKind;
use support::{
    HostFixtureOptions, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    scripted_member_client_completing, spawn_host_daemon_fixture,
};
#[cfg(feature = "test-support")]
use support::{StallGate, scripted_member_client_stalling, wait_until};

const WAIT: Duration = Duration::from_secs(30);

// ===========================================================================
// R T-15 — reconcile_resume recreates the LOCAL member and SKIPS the placed
// one: no local session is ever created for a remote id (gotcha 12)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn controlling_restart_skips_local_recreate_for_placed_members() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("resume-t15-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-t15").await;
    let report = controlling.bind_fixture(&fixture).await;

    // One local + one placed member.
    controlling
        .handle
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", "local-1"))
        .await
        .expect("local spawn");
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn");
    let placed_row = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("placed row");
    let placed_session = meerkat_core::SessionId::parse(&placed_row.session_id).expect("id");

    // Cold restart over the same storage handles.
    let controlling = controlling.restart().await;

    // Both members are in the resumed roster...
    let members = controlling.handle.list_members().await;
    for identity in ["local-1", "b2"] {
        assert!(
            members.iter().any(|entry| entry.agent_identity == identity),
            "{identity} must survive the resume, roster {members:?}"
        );
    }
    // ...the LOCAL one was locally recreated (it serves a turn from the
    // controlling realm)...
    let local = controlling
        .handle
        .member(&AgentIdentity::from("local-1"))
        .await
        .expect("local member handle");
    local
        .send("post-restart local turn", HandlingMode::Queue)
        .await
        .expect("recreated local member serves");
    // ...and the PLACED one was NOT: the controlling realm holds no session
    // for the remote id (the recreate loop skipped it — placement gate).
    assert!(
        controlling
            .service
            .load_persisted_session(&placed_session)
            .await
            .expect("controlling read")
            .is_none(),
        "reconcile_resume must never locally recreate a placed member's session"
    );
    // The placed member still serves THROUGH ITS HOST (ops owner anchored to
    // the owner bridge session; delivery rides the machine peer facts).
    let placed = controlling
        .handle
        .member(&AgentIdentity::from("b2"))
        .await
        .expect("placed member handle");
    tokio::time::timeout(
        WAIT,
        placed.send("post-restart remote turn", HandlingMode::Queue),
    )
    .await
    .expect("delivery does not hang")
    .expect("placed member serves after the controlling restart");

    fixture.shutdown().await;
}

// ===========================================================================
// #1262/#1269 - a controlling restart republishes a placed member's durable,
// host-acknowledged peer id and transport key (spawn projected them; replay
// does not), for both a Running restart and an explicit Resume of a cleanly
// Stopped mob.
// ===========================================================================

async fn assert_controlling_restart_republishes_placed_member_peer_id(stopped: bool) {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let label = if stopped {
        "resume-1262-stopped"
    } else {
        "resume-1262-running"
    };
    let fixture = spawn_host_daemon_fixture(
        HostFixtureOptions::named(&format!("{label}-host")).with_member_build(),
    )
    .await
    .expect("member-build fixture");
    let controlling = create_controlling_mob(label).await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("placed spawn");
    let placed = AgentIdentity::from("b2");
    let spawned = controlling
        .handle
        .get_member(&placed)
        .await
        .expect("read placed member")
        .expect("placed member present");
    let spawned_peer_id = spawned
        .peer_id()
        .expect("spawn projects the placed member's host-acknowledged peer id");
    let spawned_key = spawned
        .transport_public_key()
        .expect("spawn projects the placed member's transport key (#1269)")
        .to_string();
    if stopped {
        controlling.handle.stop().await.expect("clean stop");
    }

    let controlling = controlling.restart().await;
    if stopped {
        assert_eq!(
            controlling.handle.status().await.expect("status"),
            meerkat_mob::MobState::Stopped
        );
        controlling
            .handle
            .resume()
            .await
            .expect("explicit resume of the stopped mob");
    }

    let restored = controlling
        .handle
        .get_member(&placed)
        .await
        .expect("read restored placed member")
        .expect("restored placed member present");
    assert_eq!(
        restored.peer_id(),
        Some(spawned_peer_id),
        "the restored placed member must publish its durable peer id"
    );
    // Meta-style member_peer_info after restart: the same key, decodable to
    // the same peer id.
    assert_eq!(
        restored.transport_public_key(),
        Some(spawned_key.as_str()),
        "the restored placed member must republish its durable transport key"
    );
    assert_eq!(
        meerkat_comms::PubKey::from_pubkey_string(&spawned_key)
            .expect("published key decodes")
            .to_peer_id(),
        spawned_peer_id
    );

    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn running_controlling_restart_republishes_placed_member_peer_id() {
    assert_controlling_restart_republishes_placed_member_peer_id(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopped_controlling_restart_republishes_placed_member_peer_id() {
    assert_controlling_restart_republishes_placed_member_peer_id(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn controlling_restart_keeps_started_placed_autonomous_member_running() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let mut options = HostFixtureOptions::named("resume-autonomous-host").with_member_build();
    options.member_llm_client = Some(scripted_member_client_completing("kickoff-complete"));
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-autonomous").await;
    let report = controlling.bind_fixture(&fixture).await;
    let member = AgentIdentity::from("remote-autonomous");
    let mut spec = support::placed_spawn_spec("worker", member.as_str(), &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    spec.objective_id = Some(meerkat_core::interaction::ObjectiveId::new());
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("placed autonomous spawn");
    controlling
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&member), Some(WAIT))
        .await
        .expect("placed kickoff completes before restart");

    let controlling = controlling.restart().await;
    assert_eq!(
        controlling
            .handle
            .status()
            .await
            .expect("resumed mob status"),
        meerkat_mob::MobState::Running,
        "cold startup must not run local autonomous readiness against a placed peer-only member"
    );
    let ready = controlling
        .handle
        .wait_for_members_ready(std::slice::from_ref(&member), Some(WAIT))
        .await
        .expect("committed placed incarnation remains ready after restart");
    assert_eq!(ready[0].1.status, meerkat_mob::MobMemberStatus::Active);

    let placed = controlling
        .handle
        .member(&member)
        .await
        .expect("placed autonomous handle after restart");
    tokio::time::timeout(
        WAIT,
        placed.send("post-restart remote turn", HandlingMode::Queue),
    )
    .await
    .expect("post-restart delivery does not hang")
    .expect("placed autonomous member serves after controlling restart");

    fixture.shutdown().await;
}

// ===========================================================================
// R T-16 / ADJ-8 — fence-counter reseed: a post-restart respawn of a
// pre-restart member mints a fence the host admits as Superseding, never
// a spurious StaleFence
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn restart_reseeded_fence_counter_never_draws_stale_fence() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture =
        spawn_host_daemon_fixture(HostFixtureOptions::named("resume-t16-host").with_member_build())
            .await
            .expect("member-build fixture");
    let controlling = create_controlling_mob("resume-t16").await;
    let report = controlling.bind_fixture(&fixture).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("pre-restart placed spawn");
    let before = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("pre-restart row");

    let controlling = controlling.restart().await;

    // Retire + respawn the SAME identity post-restart: every fresh mint must
    // sort ABOVE the host-recorded tuple (ADJ-8: next_fence_token reseeded to
    // max(replayed fence maxima) + 1). A spurious StaleFence would fail here.
    controlling
        .handle
        .retire(AgentIdentity::from("b2"))
        .await
        .expect("post-restart retire of a pre-restart member (no spurious StaleFence)");
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("post-restart respawn admits as Superseding, never StaleFence");

    let after = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .cloned()
        .expect("post-restart row");
    assert!(
        (after.generation, after.fence_token) > (before.generation, before.fence_token),
        "the respawn tuple must sort above the pre-restart tuple: {:?} !> {:?}",
        (after.generation, after.fence_token),
        (before.generation, before.fence_token)
    );

    fixture.shutdown().await;
}

// ===========================================================================
// Ordinary placed work IN FLIGHT across a cold controlling restart.
//
// The rows above restart the controller and then send NEW work. These two
// hold one ordinary placed work (a `SubmitWork` acknowledged at turn
// completion, `MemberHandle::internal_turn`) in flight, stop the controller
// cold, and read what becomes of THAT work id.
//
// What is durable before anyone is answered: the controller's
// record-before-send event (`PlacedCompletionObligationRecorded`) and the
// member host's Pending reservation, which the host persists before its
// runtime accepts the work. The rows read both after that acceptance, with
// the member turn parked in its model call: the instant between the
// reservation and the acceptance is not separated here, and the reservation
// is still there because only a terminal consumes it.
//
// The caller's own promise is not durable, so a cold restart switches every
// pending row to exact cancellation
// (`recover_placed_completion_outcome_custody`) and never re-originates
// work. The work id then reaches exactly one terminal:
//
// - a host CANCELLATION, when the work was still running at the restart
//   (the first row);
// - the work's RESULT, when the host had already finished it and answers
//   the cancellation with the terminal it retained (the second row).
//
// Neither row is a statement that ordinary placed work survives a cold
// restart to its result: the first row pins that it does not.
// ===========================================================================

/// Counts every stream the member's runtime opens on `inner`. In these rows
/// one stream is the one model call of one admitted member turn, so the
/// count is the number of times the member host ran work.
#[cfg(feature = "test-support")]
struct StreamCountingClient {
    inner: Arc<dyn meerkat_client::LlmClient>,
    streams: Arc<AtomicUsize>,
}

#[cfg(feature = "test-support")]
#[async_trait::async_trait]
impl meerkat_client::LlmClient for StreamCountingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        self.streams.fetch_add(1, Ordering::SeqCst);
        self.inner.stream(request)
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
    }
}

/// The controller's durable custody of ordinary placed work, as its event
/// history holds it for this mob, in append order.
#[cfg(feature = "test-support")]
#[derive(Debug, Default)]
struct OrdinaryPlacedWorkHistory {
    recorded: Vec<PlacedCompletionObligationEvent>,
    cancellation_requested: Vec<PlacedCompletionObligationEvent>,
    resolved: Vec<(
        PlacedCompletionObligationEvent,
        PlacedCompletionHostOutcomeEvent,
    )>,
    closed: Vec<(
        PlacedCompletionObligationEvent,
        PlacedCompletionClosureEvent,
    )>,
    /// Folded results whose exact acknowledgement the host proved applied:
    /// the last event of a resolved work's custody.
    acknowledged: Vec<PlacedCompletionObligationEvent>,
}

#[cfg(feature = "test-support")]
impl OrdinaryPlacedWorkHistory {
    /// Terminals of every kind: a folded host result or a closure.
    fn terminal_count(&self) -> usize {
        self.resolved.len() + self.closed.len()
    }
}

#[cfg(feature = "test-support")]
async fn ordinary_placed_work_history(
    controlling: &support::ControllingMob,
) -> OrdinaryPlacedWorkHistory {
    let mut history = OrdinaryPlacedWorkHistory::default();
    let events = controlling
        .storage_events
        .replay_all()
        .await
        .expect("replay the controlling mob's event history");
    for event in events {
        if event.mob_id != controlling.mob_id {
            continue;
        }
        match event.kind {
            MobEventKind::PlacedCompletionObligationRecorded { obligation } => {
                history.recorded.push(obligation);
            }
            MobEventKind::PlacedCompletionCancellationRequested { obligation } => {
                history.cancellation_requested.push(obligation);
            }
            MobEventKind::PlacedCompletionOutcomeResolved {
                obligation,
                outcome,
            } => history.resolved.push((obligation, outcome)),
            MobEventKind::PlacedCompletionOutcomeClosed {
                obligation,
                closure,
            } => history.closed.push((obligation, closure)),
            MobEventKind::PlacedCompletionOutcomeAcknowledged { obligation } => {
                history.acknowledged.push(obligation);
            }
            _ => {}
        }
    }
    history
}

/// The caller's call of the held work.
#[cfg(feature = "test-support")]
type HeldCaller =
    tokio::task::JoinHandle<Result<meerkat_mob::MemberDeliveryReceipt, meerkat_mob::MobError>>;

/// One placed member on a host daemon whose member turns park on `gate`,
/// and one ordinary placed work submitted to it and held in flight.
#[cfg(feature = "test-support")]
struct HeldOrdinaryPlacedWork {
    fixture: support::HostDaemonFixture,
    controlling: support::ControllingMob,
    member: AgentIdentity,
    gate: StallGate,
    /// Streams the member's runtime opened: see [`StreamCountingClient`].
    streams: Arc<AtomicUsize>,
    /// The controller's record-before-send event of the held work.
    work: PlacedCompletionObligationEvent,
    /// Unanswered while the work is held.
    caller: HeldCaller,
}

/// Place one ordinary work and hold it in flight, then read the durable
/// acceptance while the caller is still unanswered: the controller's one
/// record-before-send event, and the host's one Pending reservation under
/// the same work id.
#[cfg(feature = "test-support")]
async fn hold_one_ordinary_placed_work(label: &str) -> HeldOrdinaryPlacedWork {
    let gate = StallGate::new();
    let streams = Arc::new(AtomicUsize::new(0));
    let mut options = HostFixtureOptions::named(&format!("{label}-host")).with_member_build();
    options.member_llm_client = Some(Arc::new(StreamCountingClient {
        inner: scripted_member_client_stalling(gate.clone()),
        streams: Arc::clone(&streams),
    }));
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("member-build fixture");
    let controlling = create_controlling_mob(label).await;
    let report = controlling.bind_fixture(&fixture).await;
    let member = AgentIdentity::from("held-worker");
    controlling
        .spawn_placed("worker", member.as_str(), &report.host_id)
        .await
        .expect("placed spawn");
    assert_eq!(
        streams.load(Ordering::SeqCst),
        0,
        "the placed spawn alone runs no member turn"
    );
    assert_eq!(
        ordinary_placed_work_history(&controlling)
            .await
            .recorded
            .len(),
        0,
        "no ordinary placed work is recorded before one is submitted"
    );

    let placed = controlling
        .handle
        .member(&member)
        .await
        .expect("placed member handle");
    let caller = tokio::spawn(async move {
        placed
            .internal_turn("ordinary placed work held in flight")
            .await
    });
    // The member's runtime accepted the work and is inside its one model
    // call, parked on the gate.
    tokio::time::timeout(WAIT, gate.wait_until_entered())
        .await
        .expect("the member host starts the held work");

    let history = ordinary_placed_work_history(&controlling).await;
    assert_eq!(
        history.recorded.len(),
        1,
        "the controller recorded the work once before it sent it: {history:?}"
    );
    assert_eq!(
        history.cancellation_requested.len() + history.terminal_count(),
        0,
        "nothing but the Record exists for the held work: {history:?}"
    );
    let work = history.recorded[0].clone();
    assert_eq!(work.agent_identity, member);
    assert_eq!(work.host_id, report.host_id);
    let pending = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .turn_outcome_pending
        .get(member.as_str())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        pending
            .iter()
            .map(|row| row.input_id.as_str())
            .collect::<Vec<_>>(),
        vec![work.input_id.as_str()],
        "the host holds exactly one Pending reservation, under the recorded work id"
    );
    assert_eq!(
        streams.load(Ordering::SeqCst),
        1,
        "the member host ran the held work once"
    );
    assert!(
        !caller.is_finished(),
        "the caller is still unanswered while both durable records exist"
    );

    HeldOrdinaryPlacedWork {
        fixture,
        controlling,
        member,
        gate,
        streams,
        work,
        caller,
    }
}

/// The caller of the held work, after the controller stopped cold: its
/// promise does not survive, so the call ends with an error, not a receipt.
#[cfg(feature = "test-support")]
async fn assert_caller_promise_did_not_survive(caller: HeldCaller) {
    let answered = tokio::time::timeout(WAIT, caller)
        .await
        .expect("the cold stop answers the caller")
        .expect("the caller task is not cancelled");
    assert!(
        answered.is_err(),
        "the caller's promise must not survive a cold controller stop: {answered:?}"
    );
}

/// The host journal's rows of `input_id` for `member`.
#[cfg(feature = "test-support")]
#[derive(Debug, PartialEq, Eq)]
struct HostRowsOfWork {
    /// Pending reservations.
    pending: usize,
    /// Retained terminals the controller has not yet acknowledged.
    terminals: Vec<String>,
    /// Tombstones of terminals the controller acknowledged.
    acknowledged: usize,
    /// Cancellation receipts.
    cancellations: Vec<RecordedTrackedInputCancelKind>,
}

#[cfg(feature = "test-support")]
async fn host_rows_of(
    fixture: &support::HostDaemonFixture,
    mob_id: &str,
    member: &AgentIdentity,
    input_id: &str,
) -> HostRowsOfWork {
    let record = fixture.host_binding_record(mob_id).await;
    let pending = record
        .turn_outcome_pending
        .get(member.as_str())
        .map_or(0, |rows| {
            rows.iter().filter(|row| row.input_id == input_id).count()
        });
    let terminals = record
        .turn_outcomes
        .get(member.as_str())
        .map(|rows| {
            rows.iter()
                .filter(|row| row.input_id == input_id)
                .map(|row| format!("{:?}", row.outcome))
                .collect()
        })
        .unwrap_or_default();
    let acknowledged = record
        .turn_outcome_acknowledged
        .get(member.as_str())
        .map_or(0, |rows| {
            rows.iter().filter(|row| row.input_id == input_id).count()
        });
    let cancellations = record
        .tracked_input_cancellations
        .get(member.as_str())
        .map(|rows| {
            rows.iter()
                .filter(|row| row.input_id == input_id)
                .map(|row| row.outcome)
                .collect()
        })
        .unwrap_or_default();
    HostRowsOfWork {
        pending,
        terminals,
        acknowledged,
        cancellations,
    }
}

/// Ordinary placed work still RUNNING when the controller stops cold: the
/// restarted controller cancels that exact work on its host. The work id
/// reaches exactly one terminal, a host cancellation and not a result, and
/// the member host never runs it a second time.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn cold_controlling_restart_ends_running_ordinary_placed_work_in_one_host_cancellation() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let HeldOrdinaryPlacedWork {
        fixture,
        controlling,
        member,
        gate,
        streams,
        work,
        caller,
    } = hold_one_ordinary_placed_work("resume-ordinary-cancelled").await;
    let mob_id = controlling.mob_id.to_string();

    // Stop the controller cold and restart it. The work is still running on
    // its host: the gate stays closed.
    let controlling = controlling.crash_restart().await;
    assert_caller_promise_did_not_survive(caller).await;

    wait_until(
        "the restarted controller to end the held ordinary work",
        || async {
            ordinary_placed_work_history(&controlling)
                .await
                .terminal_count()
                > 0
        },
    )
    .await;
    let history = ordinary_placed_work_history(&controlling).await;

    // (1) The SAME accepted work id is the one that reaches a terminal: one
    // Record, never a second one under any id.
    assert_eq!(
        history.recorded,
        vec![work.clone()],
        "the restart re-originated no work: {history:?}"
    );
    assert_eq!(
        history.cancellation_requested,
        vec![work.clone()],
        "cold recovery switched the pending work to exact cancellation, once: {history:?}"
    );
    // (2) The terminal's KIND and COUNT: exactly one, and it is a host
    // cancellation, not a result.
    assert_eq!(
        history.closed,
        vec![(work.clone(), PlacedCompletionClosureEvent::HostCancelled)],
        "work still running at a cold restart ends in one host cancellation: {history:?}"
    );
    assert!(
        history.resolved.is_empty(),
        "no host result is folded for work that was cancelled while running: {history:?}"
    );
    let cancelled_on_host = HostRowsOfWork {
        pending: 0,
        terminals: Vec::new(),
        acknowledged: 0,
        cancellations: vec![RecordedTrackedInputCancelKind::Cancelled],
    };
    assert_eq!(
        host_rows_of(&fixture, &mob_id, &member, &work.input_id).await,
        cancelled_on_host,
        "the host consumed the Pending reservation, journalled no result for the cancelled \
         work, and keeps one cancellation receipt for its id"
    );
    // (3) No second submission: the member host ran the work once.
    assert_eq!(
        streams.load(Ordering::SeqCst),
        1,
        "the member host never ran the cancelled work a second time"
    );

    // Later work proves the lane is live again and still never reruns the
    // cancelled id. It is sent to its end: the caller's receipt, then the
    // host's acknowledgement of its folded result, the last event of its
    // custody.
    gate.release();
    let placed = controlling
        .handle
        .member(&member)
        .await
        .expect("placed member handle after the restart");
    tokio::time::timeout(
        WAIT,
        placed.internal_turn("ordinary placed work after the restart"),
    )
    .await
    .expect("post-restart ordinary work stays bounded")
    .expect("the placed member serves ordinary work after the restart");
    wait_until(
        "the later ordinary work's result to be acknowledged",
        || async {
            !ordinary_placed_work_history(&controlling)
                .await
                .acknowledged
                .is_empty()
        },
    )
    .await;

    // The SECOND read of "exactly one", after the controller and the host
    // settled the later work: the cancelled work id still has its one
    // Record, its one cancellation request and its one terminal, the host
    // cancellation, and no result was ever folded for it. Everything else
    // in the history belongs to the later work, under another id.
    let settled = ordinary_placed_work_history(&controlling).await;
    assert_eq!(
        settled.recorded.len(),
        2,
        "the later work is its own Record: {settled:?}"
    );
    assert_eq!(settled.recorded[0], work);
    let later_work = settled.recorded[1].clone();
    assert_ne!(later_work.input_id, work.input_id);
    assert_eq!(
        settled.cancellation_requested,
        vec![work.clone()],
        "only the work in flight at the restart was ever cancelled: {settled:?}"
    );
    assert_eq!(
        settled.closed,
        vec![(work.clone(), PlacedCompletionClosureEvent::HostCancelled)],
        "the cancelled work keeps its one terminal, and nothing else is closed: {settled:?}"
    );
    assert_eq!(
        settled.resolved,
        vec![(
            later_work.clone(),
            PlacedCompletionHostOutcomeEvent::InteractionComplete
        )],
        "the only result ever folded is the later work's: {settled:?}"
    );
    assert_eq!(
        settled.acknowledged,
        vec![later_work],
        "only the later work's result is acknowledged: {settled:?}"
    );
    assert_eq!(
        host_rows_of(&fixture, &mob_id, &member, &work.input_id).await,
        cancelled_on_host,
        "the host's rows of the cancelled work are unchanged by the later work"
    );
    assert_eq!(
        streams.load(Ordering::SeqCst),
        2,
        "one more member turn ran, for the later work only"
    );

    fixture.shutdown().await;
}

/// Ordinary placed work the host FINISHED while the controller was down:
/// the restarted controller still asks for the exact cancellation, the host
/// answers with the terminal it retained, and the work id reaches exactly
/// one terminal, the work's result. The member host ran it once.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "test-support")]
async fn cold_controlling_restart_folds_the_result_of_ordinary_placed_work_its_host_had_finished() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let HeldOrdinaryPlacedWork {
        fixture,
        controlling,
        member,
        gate,
        streams,
        work,
        caller,
    } = hold_one_ordinary_placed_work("resume-ordinary-finished").await;
    let mob_id = controlling.mob_id.to_string();

    // Stop the controller cold FIRST, so nobody is left to fold a terminal,
    // then let the host finish the work on its own.
    controlling
        .handle
        .crash_stop_preserving_durable_work_for_test()
        .await
        .expect("stop the controlling mob cold, keeping its durable custody");
    assert_caller_promise_did_not_survive(caller).await;
    gate.release();
    wait_until(
        "the host to journal the finished work's terminal",
        || async {
            !host_rows_of(&fixture, &mob_id, &member, &work.input_id)
                .await
                .terminals
                .is_empty()
        },
    )
    .await;
    let stopped = ordinary_placed_work_history(&controlling).await;
    assert_eq!(
        (
            stopped.recorded.len(),
            stopped.cancellation_requested.len(),
            stopped.terminal_count()
        ),
        (1, 0, 0),
        "the stopped controller wrote nothing while its host finished the work: {stopped:?}"
    );

    let controlling = controlling.restart_after_actor_fail_stop().await;
    wait_until(
        "the restarted controller to fold the finished ordinary work",
        || async {
            ordinary_placed_work_history(&controlling)
                .await
                .terminal_count()
                > 0
        },
    )
    .await;
    let history = ordinary_placed_work_history(&controlling).await;

    // (1) The SAME accepted work id is the one that reaches a terminal.
    assert_eq!(
        history.recorded,
        vec![work.clone()],
        "the restart re-originated no work: {history:?}"
    );
    assert_eq!(
        history.cancellation_requested,
        vec![work.clone()],
        "cold recovery asks for the exact cancellation even of finished work: {history:?}"
    );
    // (2) The terminal's KIND and COUNT: exactly one, and it is the work's
    // result, not a cancellation.
    assert_eq!(
        history.resolved,
        vec![(
            work.clone(),
            PlacedCompletionHostOutcomeEvent::InteractionComplete
        )],
        "the host's retained terminal is folded once, as the work's result: {history:?}"
    );
    assert!(
        history.closed.is_empty(),
        "work the host had finished is not closed as cancelled: {history:?}"
    );
    // The host answered the cancellation of work it had finished from the
    // terminal it retained: it keeps NO cancellation receipt for the id, of
    // any kind. Its one terminal row is either still retained or already
    // moved to its acknowledgement tombstone.
    let host = host_rows_of(&fixture, &mob_id, &member, &work.input_id).await;
    assert_eq!(
        host.pending, 0,
        "the host's Pending reservation is consumed: {host:?}"
    );
    assert_eq!(
        host.cancellations,
        Vec::new(),
        "the host keeps no cancellation receipt for work it had finished: {host:?}"
    );
    assert_eq!(
        host.terminals.len() + host.acknowledged,
        1,
        "the host holds the finished work's one terminal, retained or acknowledged: {host:?}"
    );
    // (3) No second submission: the member host ran the work once.
    assert_eq!(
        streams.load(Ordering::SeqCst),
        1,
        "the member host never ran the finished work a second time"
    );

    // The SECOND read of "exactly one", after the custody's last event: the
    // host proved it applied the exact acknowledgement of the folded
    // result. Nothing was added for the work id but that acknowledgement.
    wait_until(
        "the finished ordinary work's result to be acknowledged",
        || async {
            !ordinary_placed_work_history(&controlling)
                .await
                .acknowledged
                .is_empty()
        },
    )
    .await;
    let settled = ordinary_placed_work_history(&controlling).await;
    assert_eq!(
        settled.recorded,
        vec![work.clone()],
        "still one Record: {settled:?}"
    );
    assert_eq!(
        settled.cancellation_requested,
        vec![work.clone()],
        "still one cancellation request: {settled:?}"
    );
    assert_eq!(
        settled.resolved,
        vec![(
            work.clone(),
            PlacedCompletionHostOutcomeEvent::InteractionComplete
        )],
        "still exactly one terminal, the work's result: {settled:?}"
    );
    assert!(
        settled.closed.is_empty(),
        "nothing was closed as cancelled afterwards: {settled:?}"
    );
    assert_eq!(
        settled.acknowledged,
        vec![work.clone()],
        "the result is acknowledged once: {settled:?}"
    );
    assert_eq!(
        host_rows_of(&fixture, &mob_id, &member, &work.input_id).await,
        HostRowsOfWork {
            pending: 0,
            terminals: Vec::new(),
            acknowledged: 1,
            cancellations: Vec::new(),
        },
        "the host keeps one acknowledgement tombstone for the work id and nothing else"
    );
    assert_eq!(
        streams.load(Ordering::SeqCst),
        1,
        "the member host ran the finished work once, to the end"
    );

    fixture.shutdown().await;
}
