//! Kickoff lifecycle notices leave the actor.
//!
//! MobMachine emits `EmitKickoffLifecycleNotice` for every kickoff phase of a
//! member, and the actor realizes it as one notice to each of the member's
//! wired peers. The actor prepares each notice on its own step (MobMachine's
//! verdict on it, the recipient's endpoint, the finished comms command) and
//! hands the send to a task it owns on the peer delivery lane. It never
//! awaits the send: a send to a peer that is not live waits for that peer's
//! wake, and the wake is the actor's own work. Awaited on the actor, such a
//! send held the actor until the sender's send budget ended, and every other
//! command of the mob waited behind it.
//!
//! Order and bound: the notices of one member to one peer are sent in the
//! order MobMachine emitted them. One is sent at a time, and each later one
//! waits for the one before it to settle, delivered or not, and only then
//! starts its own send with its own budget. Behind a notice that cannot be
//! delivered, the next one is therefore attempted when that one's budget has
//! ended: two notices to a peer that stays unreachable are recorded one send
//! budget apart (30 s and 60 s after the first was sent), not together. At
//! most [`MAX_PENDING_KICKOFF_NOTICES_PER_PEER`] are pending for one member
//! and one peer; a notice beyond that is refused and recorded as not
//! delivered. The notices take none of the peer message lane's permits
//! ([`MAX_PENDING_PEER_DELIVERIES`]): this bound replaces them.
//! Nothing is queued on the actor, and no fact about a kickoff lives here:
//! a notice is presentation, and a notice that is not delivered changes no
//! machine state.
//!
//! The deliveries are peer deliveries: a stop, a complete, a reset, a
//! destroy and a shutdown of the mob, an unwire of the edge and a retirement
//! of either member cancel them and wait for them to settle, as they do for
//! a pending peer message.

use super::*;

/// How many kickoff notices of one member may be pending for one peer: one
/// being sent and the rest waiting for it. One kickoff owes a peer a handful
/// of notices (pending, starting, its outcome), so only the notices of
/// several kickoffs of one identity, none of them deliverable yet, reach it.
pub(super) const MAX_PENDING_KICKOFF_NOTICES_PER_PEER: usize = 8;

/// One kickoff notice of `from` for `to`, prepared on the actor.
pub(super) struct KickoffNoticeDelivery {
    pub(super) from: AgentIdentity,
    pub(super) to: AgentIdentity,
    /// The notice's kind on the wire (`mob.kickoff_started`).
    pub(super) intent: &'static str,
    /// MobMachine held the recipient not live (Dormant or Waking) when the
    /// notice was prepared: its send wakes the recipient or joins its wake.
    pub(super) recipient_unmaterialized: bool,
    pub(super) sender_comms: Arc<dyn CoreCommsRuntime>,
    pub(super) command: CommsCommand,
}

impl MobActor {
    /// Hand one prepared kickoff notice to the peer delivery lane (see the
    /// module docs). Never waits: the notice is sent by its own task, after
    /// the pending notices of the same member to the same peer.
    pub(super) fn spawn_kickoff_notice_delivery(&mut self, notice: KickoffNoticeDelivery) {
        // Deliveries are keyed by their ticket, so the last pending notice of
        // this member to this peer is the one emitted just before this one.
        let mut pending = 0_usize;
        let mut predecessor = None;
        for delivery in self.peer_delivery_inflight.values() {
            if delivery.kickoff_notice && delivery.from == notice.from && delivery.to == notice.to {
                pending += 1;
                predecessor = Some(delivery.settled.clone());
            }
        }
        let mob_id = self.definition.id.clone();
        let limit = self.kickoff_notice_pending_limit();
        if pending >= limit {
            record_undelivered_kickoff_notice(
                &mob_id,
                &notice,
                &MobError::Internal(format!(
                    "{pending} kickoff notices of this member are already pending for this peer (limit={limit})"
                )),
            );
            return;
        }
        let id = self.next_peer_delivery_ticket;
        self.next_peer_delivery_ticket = self.next_peer_delivery_ticket.wrapping_add(1);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let task_cancel_token = cancel_token.clone();
        let (settled_tx, settled) = tokio::sync::watch::channel(false);
        let from = notice.from.clone();
        let to = notice.to.clone();
        #[cfg(all(test, not(target_arch = "wasm32")))]
        let notice = KickoffNoticeDelivery {
            sender_comms: sender_seam_for_test(
                &mob_id,
                notice.intent,
                &notice.to,
                Arc::clone(&notice.sender_comms),
            ),
            ..notice
        };
        let abort_handle = self.peer_delivery_tasks.spawn(async move {
            let result = deliver_kickoff_notice(
                Arc::clone(&notice.sender_comms),
                notice.command.clone(),
                predecessor,
                task_cancel_token,
            )
            .await;
            // A notice that was not delivered is recorded, here as it was
            // when the send was awaited on the actor.
            if let Err(error) = result {
                record_undelivered_kickoff_notice(&mob_id, &notice, &error);
            }
            settled_tx.send_replace(true);
            PeerDeliveryCompletion { id }
        });
        self.peer_delivery_inflight.insert(
            id,
            PeerDeliveryInflight {
                from,
                to,
                cancel_token,
                settled,
                kickoff_notice: true,
                #[cfg(not(target_arch = "wasm32"))]
                task_id: abort_handle.id(),
            },
        );
        #[cfg(target_arch = "wasm32")]
        let _ = abort_handle;
    }

    /// How many kickoff notices of one member may be pending for one peer.
    fn kickoff_notice_pending_limit(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = KICKOFF_NOTICE_PENDING_LIMITS_FOR_TEST
            .lock()
            .ok()
            .and_then(|limits| limits.get(&self.definition.id).copied())
        {
            return limit;
        }
        MAX_PENDING_KICKOFF_NOTICES_PER_PEER
    }
}

/// Send one kickoff notice once the notice before it settled, unless the
/// delivery is cancelled first.
///
/// `predecessor` is the settlement of the pending notice of the same member
/// to the same peer that MobMachine emitted before this one. A predecessor
/// whose task is gone (torn down with the lane) sends nothing after this
/// either, so the wait ends with it.
async fn deliver_kickoff_notice(
    sender_comms: Arc<dyn CoreCommsRuntime>,
    command: CommsCommand,
    predecessor: Option<tokio::sync::watch::Receiver<bool>>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), MobError> {
    let send = async {
        if let Some(mut predecessor) = predecessor {
            while !*predecessor.borrow_and_update() {
                if predecessor.changed().await.is_err() {
                    break;
                }
            }
        }
        sender_comms
            .send(command)
            .await
            .map(|_| ())
            .map_err(MobError::from)
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(MobError::Internal(
            "kickoff notice delivery canceled because mob topology or lifecycle changed".to_string(),
        )),
        result = send => result,
    }
}

/// Record a kickoff notice that was not delivered. A notice is presentation:
/// the record is its whole failure handling.
fn record_undelivered_kickoff_notice(
    mob_id: &MobId,
    notice: &KickoffNoticeDelivery,
    error: &MobError,
) {
    if notice.recipient_unmaterialized {
        tracing::warn!(
            %mob_id,
            from = %notice.from,
            to = %notice.to,
            intent = notice.intent,
            error = %error,
            "a kickoff notice to a peer that was not materialized was not delivered"
        );
    } else {
        tracing::warn!(
            %mob_id,
            agent_identity = %notice.from,
            to = %notice.to,
            intent = notice.intent,
            error = %error,
            "failed to emit kickoff lifecycle notice"
        );
    }
}

/// Test override of the pending bound, per mob.
#[cfg(test)]
static KICKOFF_NOTICE_PENDING_LIMITS_FOR_TEST: std::sync::Mutex<BTreeMap<MobId, usize>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Let at most `limit` kickoff notices of one member be pending for one
/// peer in `mob_id`.
#[cfg(test)]
pub(in crate::runtime) fn set_kickoff_notice_pending_limit_for_test(mob_id: &MobId, limit: usize) {
    if let Ok(mut limits) = KICKOFF_NOTICE_PENDING_LIMITS_FOR_TEST.lock() {
        limits.insert(mob_id.clone(), limit);
    }
}

/// Test seam: what a test armed for the sends of a mob's kickoff notices.
/// A seam is inside the delivery's own send, so it comes after the wait for
/// the notice before it and ends with a cancellation.
#[cfg(all(test, not(target_arch = "wasm32")))]
static KICKOFF_NOTICE_SEND_SEAMS_FOR_TEST: std::sync::Mutex<Vec<KickoffNoticeSendSeamForTest>> =
    std::sync::Mutex::new(Vec::new());

/// One armed seam: the sends of `mob_id`'s kickoff notices of kind `intent`
/// (every kind when `None`) to `to` (every recipient when `None`).
#[cfg(all(test, not(target_arch = "wasm32")))]
struct KickoffNoticeSendSeamForTest {
    mob_id: MobId,
    intent: Option<&'static str>,
    to: Option<AgentIdentity>,
    /// The send waits until this holds `true`; `None` fails the send.
    released: Option<tokio::sync::watch::Receiver<bool>>,
    arrived: Arc<std::sync::atomic::AtomicUsize>,
    passed: Arc<std::sync::atomic::AtomicUsize>,
}

/// A test's end of a held send (see [`hold_kickoff_notice_send_for_test`]).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(in crate::runtime) struct KickoffNoticeSendHoldForTest {
    released: tokio::sync::watch::Sender<bool>,
    arrived: Arc<std::sync::atomic::AtomicUsize>,
    passed: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
impl KickoffNoticeSendHoldForTest {
    /// Let the held sends go.
    pub(in crate::runtime) fn release(&self) {
        self.released.send_replace(true);
    }

    /// How many sends reached the hold: their delivery's turn had come.
    pub(in crate::runtime) fn arrived(&self) -> usize {
        self.arrived.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many sends went on to the member's own comms runtime.
    pub(in crate::runtime) fn passed(&self) -> usize {
        self.passed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Hold the send of every kickoff notice of kind `intent` in `mob_id` until
/// the returned hold is released.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(in crate::runtime) fn hold_kickoff_notice_send_for_test(
    mob_id: &MobId,
    intent: &'static str,
) -> KickoffNoticeSendHoldForTest {
    let (released, released_rx) = tokio::sync::watch::channel(false);
    let hold = KickoffNoticeSendHoldForTest {
        released,
        arrived: Arc::default(),
        passed: Arc::default(),
    };
    KICKOFF_NOTICE_SEND_SEAMS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(KickoffNoticeSendSeamForTest {
            mob_id: mob_id.clone(),
            intent: Some(intent),
            to: None,
            released: Some(released_rx),
            arrived: Arc::clone(&hold.arrived),
            passed: Arc::clone(&hold.passed),
        });
    hold
}

/// Fail the send of every kickoff notice to `to` in `mob_id` (the peer is
/// reported offline), as a send to a live recipient can fail.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(in crate::runtime) fn fail_kickoff_notice_send_for_test(mob_id: &MobId, to: &AgentIdentity) {
    KICKOFF_NOTICE_SEND_SEAMS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(KickoffNoticeSendSeamForTest {
            mob_id: mob_id.clone(),
            intent: None,
            to: Some(to.clone()),
            released: None,
            arrived: Arc::default(),
            passed: Arc::default(),
        });
}

/// `sender`, or `sender` behind the seam armed for `mob_id`, `intent` and
/// `to`.
#[cfg(all(test, not(target_arch = "wasm32")))]
fn sender_seam_for_test(
    mob_id: &MobId,
    intent: &str,
    to: &AgentIdentity,
    sender: Arc<dyn CoreCommsRuntime>,
) -> Arc<dyn CoreCommsRuntime> {
    let seams = KICKOFF_NOTICE_SEND_SEAMS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let armed = seams.iter().find(|seam| {
        &seam.mob_id == mob_id
            && seam.intent.is_none_or(|armed| armed == intent)
            && seam.to.as_ref().is_none_or(|armed| armed == to)
    });
    match armed {
        Some(seam) => Arc::new(SeamedSenderForTest {
            sender,
            released: seam.released.clone(),
            arrived: Arc::clone(&seam.arrived),
            passed: Arc::clone(&seam.passed),
        }),
        None => sender,
    }
}

/// A sender whose sends wait for the test's release, or fail.
#[cfg(all(test, not(target_arch = "wasm32")))]
struct SeamedSenderForTest {
    sender: Arc<dyn CoreCommsRuntime>,
    released: Option<tokio::sync::watch::Receiver<bool>>,
    arrived: Arc<std::sync::atomic::AtomicUsize>,
    passed: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[async_trait::async_trait]
impl CoreCommsRuntime for SeamedSenderForTest {
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.sender.inbox_notify()
    }

    async fn send(&self, cmd: CommsCommand) -> Result<meerkat_core::comms::SendReceipt, SendError> {
        self.arrived
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(mut released) = self.released.clone() else {
            return Err(SendError::PeerOffline);
        };
        while !*released.borrow_and_update() {
            if released.changed().await.is_err() {
                break;
            }
        }
        self.passed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sender.send(cmd).await
    }
}

/// Test seam: the recipients whose kickoff notices fail their preparation on
/// the actor, per mob.
#[cfg(test)]
static KICKOFF_NOTICE_PREPARATION_FAULTS_FOR_TEST: std::sync::Mutex<Vec<(MobId, AgentIdentity)>> =
    std::sync::Mutex::new(Vec::new());

/// Fail the preparation of every kickoff notice to `to` in `mob_id` where
/// its endpoint is resolved.
#[cfg(test)]
pub(in crate::runtime) fn fail_kickoff_notice_preparation_for_test(
    mob_id: &MobId,
    to: &AgentIdentity,
) {
    KICKOFF_NOTICE_PREPARATION_FAULTS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((mob_id.clone(), to.clone()));
}

/// The fault armed for the preparation of a kickoff notice to `to` in
/// `mob_id`, if any.
#[cfg(test)]
pub(super) fn kickoff_notice_preparation_fault_for_test(
    mob_id: &MobId,
    to: &AgentIdentity,
) -> Result<(), MobError> {
    let armed = KICKOFF_NOTICE_PREPARATION_FAULTS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|(armed_mob, armed_to)| armed_mob == mob_id && armed_to == to);
    if armed {
        return Err(MobError::WiringError(format!(
            "fault-injected endpoint resolution failure for kickoff notice recipient '{to}'"
        )));
    }
    Ok(())
}

/// Test seam: the members whose spawn fails where it prepares its kickoff
/// input, after its starting notice was handed to the lane.
#[cfg(test)]
static AUTONOMOUS_KICKOFF_START_FAULTS_FOR_TEST: std::sync::Mutex<Vec<AgentIdentity>> =
    std::sync::Mutex::new(Vec::new());

/// Fail the spawn of `member` where it prepares its kickoff input.
#[cfg(test)]
pub(in crate::runtime) fn fail_autonomous_kickoff_start_for_test(member: &AgentIdentity) {
    AUTONOMOUS_KICKOFF_START_FAULTS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(member.clone());
}

/// The fault armed for `member`'s kickoff start, if any.
#[cfg(test)]
pub(super) fn autonomous_kickoff_start_fault_for_test(
    member: &AgentIdentity,
) -> Result<(), MobError> {
    let armed = AUTONOMOUS_KICKOFF_START_FAULTS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(member);
    if armed {
        return Err(MobError::Internal(format!(
            "fault-injected kickoff start failure for '{member}'"
        )));
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::comms::{PeerId, SendReceipt};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A sender that counts its sends and holds each until its gate opens.
    struct CountingSender {
        notify: Arc<tokio::sync::Notify>,
        sends: AtomicUsize,
        gate: tokio::sync::watch::Sender<bool>,
    }

    impl CountingSender {
        fn new(open: bool) -> Arc<Self> {
            let (gate, _) = tokio::sync::watch::channel(open);
            Arc::new(Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                sends: AtomicUsize::new(0),
                gate,
            })
        }

        fn sends(&self) -> usize {
            self.sends.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for CountingSender {
        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }

        async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            let mut gate = self.gate.subscribe();
            while !*gate.borrow_and_update() {
                if gate.changed().await.is_err() {
                    break;
                }
            }
            Ok(SendReceipt::PeerLifecycleSent {
                envelope_id: uuid::Uuid::new_v4(),
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
            })
        }
    }

    fn started_notice() -> CommsCommand {
        CommsCommand::PeerLifecycle {
            to: PeerRoute::new(PeerId::new()),
            kind: PeerLifecycleKind::KickoffStarted,
            params: serde_json::json!({ "peer": "member" }),
        }
    }

    /// Let the spawned delivery run as far as it can.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    /// A notice is not sent while the notice before it is pending, and is
    /// sent once that one settled.
    #[tokio::test]
    async fn a_kickoff_notice_is_sent_only_after_the_one_before_it_settled() {
        let sender = CountingSender::new(true);
        let (settled_tx, settled_rx) = tokio::sync::watch::channel(false);
        let delivery = tokio::spawn(deliver_kickoff_notice(
            sender.clone(),
            started_notice(),
            Some(settled_rx),
            tokio_util::sync::CancellationToken::new(),
        ));
        settle().await;
        assert_eq!(
            sender.sends(),
            0,
            "the notice waits while the notice before it is pending"
        );
        settled_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("the notice is sent once the one before it settled")
            .expect("the delivery task joins")
            .expect("the notice is delivered");
        assert_eq!(sender.sends(), 1, "the notice is sent once");
    }

    /// A notice whose predecessor's task is gone without settling (torn down
    /// with the lane) does not wait for it.
    #[tokio::test]
    async fn a_kickoff_notice_does_not_wait_for_a_predecessor_that_is_gone() {
        let sender = CountingSender::new(true);
        let (settled_tx, settled_rx) = tokio::sync::watch::channel(false);
        drop(settled_tx);
        tokio::time::timeout(
            Duration::from_secs(5),
            deliver_kickoff_notice(
                sender.clone(),
                started_notice(),
                Some(settled_rx),
                tokio_util::sync::CancellationToken::new(),
            ),
        )
        .await
        .expect("the notice does not wait for a predecessor that is gone")
        .expect("the notice is delivered");
        assert_eq!(sender.sends(), 1);
    }

    /// A cancelled delivery ends at once and sends nothing more: while it
    /// waits for the notice before it, and while its own send is pending.
    #[tokio::test]
    async fn a_cancelled_kickoff_notice_delivery_ends_at_once() {
        // Cancelled while it waits for the notice before it: never sent.
        let sender = CountingSender::new(true);
        let (_settled_tx, settled_rx) = tokio::sync::watch::channel(false);
        let cancel = tokio_util::sync::CancellationToken::new();
        let delivery = tokio::spawn(deliver_kickoff_notice(
            sender.clone(),
            started_notice(),
            Some(settled_rx),
            cancel.clone(),
        ));
        settle().await;
        cancel.cancel();
        let cancelled = tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("a cancelled delivery ends while it waits for its predecessor")
            .expect("the delivery task joins");
        assert!(cancelled.is_err(), "a cancelled delivery is not delivered");
        assert_eq!(
            sender.sends(),
            0,
            "a cancelled waiting notice is never sent"
        );

        // Cancelled while its own send is pending: the send is dropped.
        let sender = CountingSender::new(false);
        let cancel = tokio_util::sync::CancellationToken::new();
        let delivery = tokio::spawn(deliver_kickoff_notice(
            sender.clone(),
            started_notice(),
            None,
            cancel.clone(),
        ));
        settle().await;
        assert_eq!(sender.sends(), 1, "the send is pending");
        cancel.cancel();
        let cancelled = tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("a cancelled delivery ends while its send is pending")
            .expect("the delivery task joins");
        assert!(cancelled.is_err(), "a cancelled send is not delivered");
    }
}
