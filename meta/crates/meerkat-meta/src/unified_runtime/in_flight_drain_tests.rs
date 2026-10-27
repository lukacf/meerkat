//! [`UnifiedRuntime::drain_in_flight_turns`] over a member whose turn its
//! model holds mid-flight. A turn that ends inside the window is reported
//! ended with its terminal. One still running at the bound is reported so
//! and left running: the drain cancels nothing, and the shutdown that
//! follows settles it as it does without a drain. This fixture's sessions
//! are ephemeral, so the members are not durable and the shutdown cancels
//! their turns; a durable member's turn is kept instead (the graceful
//! shutdown rows in `tests/identity_first_cold_restart_continuity.rs`).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::turn_execution_authority::TurnTerminalOutcome;
use meerkat_core::{Message, Provider};
use meerkat_mob::{AgentIdentity, MemberRunDrain, MobHandle, MobState};

use super::{InFlightDrainOutcome, InFlightDrainReport, UnifiedRuntime};

/// The bound of every wait on the fixture itself.
const WAIT: Duration = Duration::from_secs(30);

/// The model calls a [`GatedClient`] has seen.
#[derive(Default)]
pub(crate) struct ModelCalls {
    /// Calls held at the gate right now.
    pub(crate) waiting: AtomicUsize,
    /// Calls that passed the gate and answered.
    pub(crate) answered: AtomicUsize,
}

/// Takes a call out of [`ModelCalls::waiting`] when it leaves the gate,
/// answered or dropped.
struct Waiting(Arc<ModelCalls>);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A model whose every call holds until the gate opens, then answers one
/// complete turn.
struct GatedClient {
    gate: tokio::sync::watch::Receiver<bool>,
    calls: Arc<ModelCalls>,
}

#[async_trait::async_trait]
impl LlmClient for GatedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        self.calls.waiting.fetch_add(1, Ordering::SeqCst);
        let waiting = Waiting(Arc::clone(&self.calls));
        let calls = Arc::clone(&self.calls);
        let mut gate = self.gate.clone();
        Box::pin(async_stream::try_stream! {
            let _ = gate.wait_for(|open| *open).await;
            calls.answered.fetch_add(1, Ordering::SeqCst);
            drop(waiting);
            yield LlmEvent::TextDelta { delta: "done".to_string(), meta: None };
            yield LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI, &request.model, meerkat_core::Usage::default(),
                ),
            };
            yield LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A runtime whose members' model calls hold until `gate` opens.
pub(crate) async fn gated_runtime(
    mob_id: &str,
    gate: tokio::sync::watch::Receiver<bool>,
    calls: Arc<ModelCalls>,
) -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
         [profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("mob definition");
    UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(GatedClient { gate, calls }))
        .build()
        .await
        .expect("runtime builds")
}

/// Seat `member`, an autonomous member: its kickoff turn starts at once and
/// holds at the gate. Returns once that turn's model call is waiting.
pub(crate) async fn seat_mid_turn(
    handle: &MobHandle,
    calls: &ModelCalls,
    member: &str,
) -> AgentIdentity {
    let identity = AgentIdentity::from(member);
    handle
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            identity.clone(),
        ))
        .await
        .expect("seat the member");
    tokio::time::timeout(WAIT, async {
        while calls.waiting.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the member's turn reaches its model");
    identity
}

pub(crate) async fn wait_until_stopped(handle: &MobHandle) {
    tokio::time::timeout(WAIT, async {
        while handle.status().await.expect("mob status") != MobState::Stopped {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the drain stops the mob");
}

/// `member`'s run as the drain reported it.
fn drained_run<'a>(report: &'a InFlightDrainReport, member: &AgentIdentity) -> &'a MemberRunDrain {
    let InFlightDrainOutcome::Drained(drained) = &report.outcome else {
        panic!("the drain ran: {report:?}");
    };
    &drained
        .members
        .get(member)
        .unwrap_or_else(|| panic!("{member} is reported: {drained:?}"))
        .run
}

/// The drain stops the mob with the member's turn left running, and that
/// turn completes inside the window: the drain returns when it ends, well
/// before the window does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_lets_a_members_in_flight_turn_complete() {
    let (open, gate) = tokio::sync::watch::channel(false);
    let calls = Arc::new(ModelCalls::default());
    let runtime = gated_runtime("drain-completes", gate, Arc::clone(&calls)).await;
    let handle = runtime.mob_handle();
    let member = seat_mid_turn(&handle, &calls, "worker-1").await;

    let window = Duration::from_mins(1);
    let (report, ()) = tokio::join!(
        runtime.drain_in_flight_turns(Instant::now() + window),
        async {
            // Released only once the drain's Stop has left the turn running.
            wait_until_stopped(&handle).await;
            open.send(true).expect("open the gate");
        }
    );

    assert!(
        matches!(
            drained_run(&report, &member),
            MemberRunDrain::Ended {
                terminal: Some(TurnTerminalOutcome::Completed),
                ..
            }
        ),
        "the turn completes inside the window: {report:?}"
    );
    assert_eq!(calls.answered.load(Ordering::SeqCst), 1);
    assert!(
        report.waited < window / 2,
        "the drain returns when the turn ends, not at its bound: {report:?}"
    );
    assert_eq!(
        handle.status().await.expect("mob status"),
        MobState::Stopped
    );
    runtime.shutdown().await;
}

/// A turn still running at the bound is reported so. The drain cancels
/// nothing: the turn's model call is still waiting after it returns, and
/// only the shutdown that follows ends it. The drain stops the actor-loop
/// probe, so the long turn pages no stall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_at_its_bound_leaves_the_turn_to_the_shutdown() {
    let (_open, gate) = tokio::sync::watch::channel(false);
    let calls = Arc::new(ModelCalls::default());
    let runtime = gated_runtime("drain-bound", gate, Arc::clone(&calls)).await;
    let handle = runtime.mob_handle();
    let member = seat_mid_turn(&handle, &calls, "worker-1").await;

    assert!(
        runtime.actor_loop_probe_task.lock().await.is_some(),
        "the runtime probes its actor loop"
    );
    let window = Duration::from_millis(500);
    let report = runtime.drain_in_flight_turns(Instant::now() + window).await;
    assert!(
        runtime.actor_loop_probe_task.lock().await.is_none(),
        "the drain stops the actor-loop probe: a long drained turn pages no stall"
    );

    assert!(
        matches!(
            drained_run(&report, &member),
            MemberRunDrain::StillRunning { .. }
        ),
        "{report:?}"
    );
    assert!(report.waited >= window, "held to its bound: {report:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        calls.waiting.load(Ordering::SeqCst),
        1,
        "the drain left the turn's model call running"
    );

    tokio::time::timeout(WAIT, runtime.shutdown())
        .await
        .expect("the shutdown ends the turn and finishes");
    assert_eq!(
        (
            calls.waiting.load(Ordering::SeqCst),
            calls.answered.load(Ordering::SeqCst)
        ),
        (0, 0),
        "the shutdown cancelled the turn's model call"
    );
}

/// Without a drain window the shutdown's own stop leaves the member's run
/// running, as a zero-window drain would, and the terminal Shutdown settles
/// it. This member's session is ephemeral, so it is not durable and its turn
/// still ends at the shutdown: its model call is dropped unanswered. Runs on
/// any meerkat: an earlier meerkat's Shutdown cancels every running turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_without_a_drain_ends_a_non_durable_members_turn() {
    let (_open, gate) = tokio::sync::watch::channel(false);
    let calls = Arc::new(ModelCalls::default());
    let runtime = gated_runtime("shutdown-no-drain", gate, Arc::clone(&calls)).await;
    let handle = runtime.mob_handle();
    seat_mid_turn(&handle, &calls, "worker-1").await;

    tokio::time::timeout(WAIT, runtime.shutdown())
        .await
        .expect("the shutdown ends the turn and finishes");
    assert_eq!(
        (
            calls.waiting.load(Ordering::SeqCst),
            calls.answered.load(Ordering::SeqCst)
        ),
        (0, 0),
        "the shutdown ended the turn's model call unanswered"
    );
}
