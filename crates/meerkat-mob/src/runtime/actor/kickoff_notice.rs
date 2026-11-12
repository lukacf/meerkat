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
//! Order: the notices of one member to one peer are sent in the order
//! MobMachine emitted them, one at a time. They share one queue
//! ([`KickoffNoticeQueue`]) and one task that drains it: the next notice is
//! sent only when the one before it settled, delivered or not, and starts
//! its own send with its own budget then. The budget is the member's own
//! send budget (for an in-process peer its comms ack timeout, 30 seconds by
//! default); the lane adds none. Behind a notice that cannot be delivered,
//! the next one is therefore attempted when that one's budget has ended: two
//! notices to a peer that stays unreachable are recorded one send budget
//! apart, not together. Notices to different peers, and notices of different
//! members, are independent: a peer that cannot be reached delays only the
//! notices owed to it.
//!
//! Bound: at most [`MAX_PENDING_KICKOFF_NOTICES_PER_PEER`] notices of one
//! member are pending for one peer, the one being sent included. A notice
//! owed beyond that takes the place of the oldest one still queued, which is
//! dropped and recorded; the one being sent is never dropped. The latest
//! phase therefore survives (a terminal one included), and the notices that
//! stay keep their order. There is no bound across peers or members: the
//! lane holds at most that many notices for every wired pair. The notices
//! take none of the peer message lane's permits
//! ([`MAX_PENDING_PEER_DELIVERIES`]): this bound replaces them.
//!
//! One rule for a notice that is not delivered: it is recorded at WARN with
//! the member, the peer, the notice's kind and a class
//! ([`KickoffNoticeLoss`]), nothing is retried, and the member's other peers
//! and later notices are still served. That holds for a failed preparation
//! on the actor, a failed send, a notice dropped at the bound and a
//! cancelled one alike. No fact about a kickoff lives here: a notice is
//! presentation, and a notice that is not delivered changes no machine
//! state.
//!
//! The deliveries are peer deliveries: a stop, a complete, a reset, a
//! destroy and a shutdown of the mob, an unwire of the edge, a retirement of
//! either member and the rollback of a failed spawn cancel the notices that
//! are pending when they start, and wait for them to settle, as they do for
//! a pending peer message. A notice such a command emits itself (the
//! cancellation of a kickoff a stop closes) is handed to the lane after that
//! cancel: it ends within its own send budget, and the actor's exit ends the
//! lane, so nothing is delivered once a mob is destroyed.

use super::*;

/// How many kickoff notices of one member may be pending for one peer: one
/// being sent and the rest queued behind it. One kickoff owes a peer a
/// handful of notices (pending, starting, its outcome), so only the notices
/// of several kickoffs of one identity, none of them deliverable yet, reach
/// it.
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

/// Why a kickoff notice was not delivered: the `class` of its record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KickoffNoticeLoss {
    /// Its preparation on the actor failed (MobMachine's verdict, the
    /// recipient's endpoint, the command): it never reached the lane.
    PreparationFailed,
    /// Its send failed, or the send's budget ended.
    SendFailed,
    /// It was the oldest queued notice of its member and peer when another
    /// was owed at the pending bound.
    PendingBound,
    /// A lifecycle or topology change cancelled it.
    Cancelled,
}

impl KickoffNoticeLoss {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::PreparationFailed => "preparation_failed",
            Self::SendFailed => "send_failed",
            Self::PendingBound => "pending_bound",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The kickoff notices of one member that are pending for one peer: the one
/// being sent and those queued behind it, oldest first. The actor offers
/// notices on its own step; the pair's delivery task takes them one at a
/// time. Neither waits for the other.
pub(super) struct KickoffNoticeQueue {
    state: std::sync::Mutex<KickoffNoticeQueueState>,
}

struct KickoffNoticeQueueState {
    queued: VecDeque<KickoffNoticeDelivery>,
    /// The delivery task holds one notice it took and is sending it.
    sending: bool,
    /// The delivery task ended: the queue takes no further notice.
    closed: bool,
}

/// What became of a notice offered to a pair's queue.
enum KickoffNoticeOffer {
    /// It is queued. `dropped` is the notice that left the queue to keep the
    /// pending bound: the oldest queued one, which is the offered notice
    /// itself when nothing else is queued.
    Queued {
        dropped: Option<KickoffNoticeDelivery>,
    },
    /// The queue's delivery task ended: the notice needs a new queue.
    Closed(KickoffNoticeDelivery),
}

impl KickoffNoticeQueue {
    /// An open queue that holds `first`, not yet being sent.
    fn starting_with(first: KickoffNoticeDelivery) -> Self {
        Self {
            state: std::sync::Mutex::new(KickoffNoticeQueueState {
                queued: VecDeque::from([first]),
                sending: false,
                closed: false,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, KickoffNoticeQueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue `notice` behind the pending ones. With more than `limit`
    /// pending, the oldest queued notice leaves; the one being sent stays.
    fn offer(&self, notice: KickoffNoticeDelivery, limit: usize) -> KickoffNoticeOffer {
        let mut state = self.state();
        if state.closed {
            return KickoffNoticeOffer::Closed(notice);
        }
        state.queued.push_back(notice);
        let pending = state.queued.len() + usize::from(state.sending);
        let dropped = if pending > limit {
            state.queued.pop_front()
        } else {
            None
        };
        KickoffNoticeOffer::Queued { dropped }
    }

    /// The next notice to send, the oldest queued one. With none queued the
    /// queue closes: the delivery task ends.
    fn next(&self) -> Option<KickoffNoticeDelivery> {
        let mut state = self.state();
        let next = state.queued.pop_front();
        state.sending = next.is_some();
        state.closed = next.is_none();
        next
    }

    /// Close the queue and take what is still queued.
    fn close(&self) -> Vec<KickoffNoticeDelivery> {
        let mut state = self.state();
        state.sending = false;
        state.closed = true;
        state.queued.drain(..).collect()
    }
}

impl MobActor {
    /// Hand one prepared kickoff notice to the peer delivery lane (see the
    /// module docs). Never waits: the notice joins the queue of its member
    /// and peer, or starts that queue and the task that sends from it.
    pub(super) fn spawn_kickoff_notice_delivery(&mut self, notice: KickoffNoticeDelivery) {
        let mob_id = self.definition.id.clone();
        let limit = self.kickoff_notice_pending_limit();
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
        // Deliveries are keyed by their ticket, so the last queue of this
        // member and this peer is the newest one: the only one that can
        // still be open.
        let pair_queue = self
            .peer_delivery_inflight
            .values()
            .rev()
            .filter(|delivery| delivery.from == notice.from && delivery.to == notice.to)
            .find_map(|delivery| delivery.kickoff_notices.clone());
        let offer = match pair_queue {
            Some(queue) => queue.offer(notice, limit),
            None => KickoffNoticeOffer::Closed(notice),
        };
        let notice = match offer {
            KickoffNoticeOffer::Queued { dropped } => {
                if let Some(dropped) = dropped {
                    record_undelivered_kickoff_notice(
                        &mob_id,
                        &dropped,
                        KickoffNoticeLoss::PendingBound,
                        &MobError::Internal(format!(
                            "the kickoff notices of this member pending for this peer reached their bound (limit={limit}): the oldest queued one was dropped"
                        )),
                    );
                }
                return;
            }
            // No queue of this pair is open: the notice starts one.
            KickoffNoticeOffer::Closed(notice) => notice,
        };
        let id = self.next_peer_delivery_ticket;
        self.next_peer_delivery_ticket = self.next_peer_delivery_ticket.wrapping_add(1);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let task_cancel_token = cancel_token.clone();
        let (settled_tx, settled) = tokio::sync::watch::channel(false);
        let from = notice.from.clone();
        let to = notice.to.clone();
        let queue = Arc::new(KickoffNoticeQueue::starting_with(notice));
        let task_queue = Arc::clone(&queue);
        let abort_handle = self.peer_delivery_tasks.spawn(async move {
            deliver_kickoff_notices(&mob_id, &task_queue, task_cancel_token).await;
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
                kickoff_notices: Some(queue),
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

/// Send the kickoff notices of one member to one peer, one at a time and
/// oldest first, until the queue is empty or the delivery is cancelled.
///
/// A notice that is not delivered is recorded and the next one is sent. A
/// cancellation ends the task at once: the notice being sent and every
/// queued one are recorded as cancelled, and none of them is sent.
async fn deliver_kickoff_notices(
    mob_id: &MobId,
    queue: &KickoffNoticeQueue,
    cancel: tokio_util::sync::CancellationToken,
) {
    while let Some(notice) = queue.next() {
        let sent = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            sent = notice.sender_comms.send(notice.command.clone()) => Some(sent),
        };
        match sent {
            Some(Ok(_)) => {}
            Some(Err(error)) => record_undelivered_kickoff_notice(
                mob_id,
                &notice,
                KickoffNoticeLoss::SendFailed,
                &MobError::from(error),
            ),
            None => {
                let cancelled = MobError::Internal(
                    "kickoff notice delivery canceled because mob topology or lifecycle changed"
                        .to_string(),
                );
                record_undelivered_kickoff_notice(
                    mob_id,
                    &notice,
                    KickoffNoticeLoss::Cancelled,
                    &cancelled,
                );
                for queued in queue.close() {
                    record_undelivered_kickoff_notice(
                        mob_id,
                        &queued,
                        KickoffNoticeLoss::Cancelled,
                        &cancelled,
                    );
                }
                return;
            }
        }
    }
}

/// Record a kickoff notice that was not delivered. A notice is presentation:
/// the record is its whole failure handling.
fn record_undelivered_kickoff_notice(
    mob_id: &MobId,
    notice: &KickoffNoticeDelivery,
    class: KickoffNoticeLoss,
    error: &MobError,
) {
    if notice.recipient_unmaterialized {
        tracing::warn!(
            %mob_id,
            from = %notice.from,
            to = %notice.to,
            intent = notice.intent,
            class = class.as_str(),
            error = %error,
            "a kickoff notice to a peer that was not materialized was not delivered"
        );
    } else {
        tracing::warn!(
            %mob_id,
            from = %notice.from,
            to = %notice.to,
            intent = notice.intent,
            class = class.as_str(),
            error = %error,
            "a kickoff notice to a live peer was not delivered"
        );
    }
}

/// Record a kickoff notice of `from` for `to` whose preparation on the actor
/// failed: it was never handed to the lane.
pub(super) fn record_unprepared_kickoff_notice(
    mob_id: &MobId,
    from: &AgentIdentity,
    to: &AgentIdentity,
    intent: &'static str,
    error: &MobError,
) {
    tracing::warn!(
        %mob_id,
        %from,
        %to,
        intent,
        class = KickoffNoticeLoss::PreparationFailed.as_str(),
        error = %error,
        "a kickoff notice could not be prepared for a peer and was not delivered"
    );
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

    /// A sender that records the kind of every send as it starts and holds
    /// each send until the test lets one more through.
    struct RecordingSender {
        notify: Arc<tokio::sync::Notify>,
        started: std::sync::Mutex<Vec<PeerLifecycleKind>>,
        gate: tokio::sync::Semaphore,
    }

    impl RecordingSender {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                notify: Arc::new(tokio::sync::Notify::new()),
                started: std::sync::Mutex::new(Vec::new()),
                gate: tokio::sync::Semaphore::new(0),
            })
        }

        /// The kinds of the sends that started, in order.
        fn started(&self) -> Vec<PeerLifecycleKind> {
            self.started.lock().expect("recorded sends").clone()
        }

        fn let_through(&self, sends: usize) {
            self.gate.add_permits(sends);
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for RecordingSender {
        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }

        async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            if let CommsCommand::PeerLifecycle { kind, .. } = &cmd {
                self.started.lock().expect("recorded sends").push(*kind);
            }
            self.gate
                .acquire()
                .await
                .expect("the gate stays open")
                .forget();
            Ok(SendReceipt::PeerLifecycleSent {
                envelope_id: uuid::Uuid::new_v4(),
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
            })
        }
    }

    fn notice(
        sender: &Arc<RecordingSender>,
        kind: PeerLifecycleKind,
        intent: &'static str,
    ) -> KickoffNoticeDelivery {
        KickoffNoticeDelivery {
            from: AgentIdentity::from("member"),
            to: AgentIdentity::from("peer"),
            intent,
            recipient_unmaterialized: false,
            sender_comms: sender.clone(),
            command: CommsCommand::PeerLifecycle {
                to: PeerRoute::new(PeerId::new()),
                kind,
                params: serde_json::json!({ "peer": "member" }),
            },
        }
    }

    fn starting(sender: &Arc<RecordingSender>) -> KickoffNoticeDelivery {
        notice(
            sender,
            PeerLifecycleKind::KickoffStarting,
            "mob.kickoff_starting",
        )
    }

    fn callback_pending(sender: &Arc<RecordingSender>) -> KickoffNoticeDelivery {
        notice(
            sender,
            PeerLifecycleKind::KickoffCallbackPending,
            "mob.kickoff_callback_pending",
        )
    }

    fn started(sender: &Arc<RecordingSender>) -> KickoffNoticeDelivery {
        notice(
            sender,
            PeerLifecycleKind::KickoffStarted,
            "mob.kickoff_started",
        )
    }

    fn failed(sender: &Arc<RecordingSender>) -> KickoffNoticeDelivery {
        notice(
            sender,
            PeerLifecycleKind::KickoffFailed,
            "mob.kickoff_failed",
        )
    }

    /// The intent of the notice an offer dropped, if it dropped one.
    fn dropped_by(offer: KickoffNoticeOffer) -> Option<&'static str> {
        match offer {
            KickoffNoticeOffer::Queued { dropped } => dropped.map(|notice| notice.intent),
            KickoffNoticeOffer::Closed(_) => panic!("the queue is open"),
        }
    }

    /// Let the spawned delivery run as far as it can.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    /// The notices of one queue are sent one at a time, oldest first: a
    /// notice is not sent while the one before it is pending, and the queue
    /// closes with its task.
    #[tokio::test]
    async fn queued_kickoff_notices_are_sent_one_at_a_time_oldest_first() {
        let sender = RecordingSender::new();
        let queue = Arc::new(KickoffNoticeQueue::starting_with(starting(&sender)));
        assert_eq!(dropped_by(queue.offer(callback_pending(&sender), 8)), None);
        assert_eq!(dropped_by(queue.offer(failed(&sender), 8)), None);
        let delivery = tokio::spawn({
            let queue = Arc::clone(&queue);
            async move {
                deliver_kickoff_notices(
                    &MobId::from("lane"),
                    &queue,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await;
            }
        });
        settle().await;
        assert_eq!(
            sender.started(),
            [PeerLifecycleKind::KickoffStarting],
            "the second notice waits while the first is being sent"
        );
        sender.let_through(1);
        settle().await;
        assert_eq!(
            sender.started(),
            [
                PeerLifecycleKind::KickoffStarting,
                PeerLifecycleKind::KickoffCallbackPending
            ],
            "the third notice waits while the second is being sent"
        );
        sender.let_through(2);
        tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("the delivery ends once its queue is empty")
            .expect("the delivery task joins");
        assert_eq!(
            sender.started(),
            [
                PeerLifecycleKind::KickoffStarting,
                PeerLifecycleKind::KickoffCallbackPending,
                PeerLifecycleKind::KickoffFailed
            ],
            "each notice is sent once, in the order it was queued"
        );
        assert!(
            matches!(
                queue.offer(started(&sender), 8),
                KickoffNoticeOffer::Closed(_)
            ),
            "a queue whose task ended takes no further notice"
        );
    }

    /// At the bound the notice that leaves is the oldest queued one, never
    /// the one being sent; the ones that stay keep their order. With nothing
    /// else queued, the notice owed is the one that leaves.
    #[test]
    fn at_the_bound_the_oldest_queued_notice_leaves_and_never_the_one_being_sent() {
        let sender = RecordingSender::new();
        let queue = KickoffNoticeQueue::starting_with(starting(&sender));
        let sending = queue.next().expect("the first notice is taken");
        assert_eq!(sending.intent, "mob.kickoff_starting");
        assert_eq!(dropped_by(queue.offer(callback_pending(&sender), 3)), None);
        assert_eq!(
            dropped_by(queue.offer(started(&sender), 3)),
            None,
            "three pending (one being sent, two queued) are within a bound of three"
        );
        assert_eq!(
            dropped_by(queue.offer(failed(&sender), 3)),
            Some("mob.kickoff_callback_pending"),
            "the fourth takes the place of the oldest queued notice"
        );
        let rest: Vec<&'static str> =
            std::iter::from_fn(|| queue.next().map(|notice| notice.intent)).collect();
        assert_eq!(
            rest,
            ["mob.kickoff_started", "mob.kickoff_failed"],
            "the notices that stayed keep their order, the latest last"
        );

        let queue = KickoffNoticeQueue::starting_with(starting(&sender));
        let _sending = queue.next().expect("the first notice is taken");
        assert_eq!(
            dropped_by(queue.offer(started(&sender), 1)),
            Some("mob.kickoff_started"),
            "with a bound of one the notice being sent stays and the one owed leaves"
        );
    }

    /// A cancelled delivery ends at once and sends nothing more: the send in
    /// flight is dropped, the queued notices are never sent, and the queue
    /// closes. A delivery cancelled before its first send sends nothing.
    #[tokio::test]
    async fn a_cancelled_kickoff_notice_delivery_ends_at_once_and_sends_nothing_more() {
        let sender = RecordingSender::new();
        let queue = Arc::new(KickoffNoticeQueue::starting_with(starting(&sender)));
        assert_eq!(dropped_by(queue.offer(started(&sender), 8)), None);
        let cancel = tokio_util::sync::CancellationToken::new();
        let delivery = tokio::spawn({
            let queue = Arc::clone(&queue);
            let cancel = cancel.clone();
            async move {
                deliver_kickoff_notices(&MobId::from("lane"), &queue, cancel).await;
            }
        });
        settle().await;
        assert_eq!(
            sender.started(),
            [PeerLifecycleKind::KickoffStarting],
            "the first send is pending"
        );
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("a cancelled delivery ends while its send is pending")
            .expect("the delivery task joins");
        sender.let_through(2);
        settle().await;
        assert_eq!(
            sender.started(),
            [PeerLifecycleKind::KickoffStarting],
            "a cancelled delivery never sends its queued notices"
        );
        assert!(
            matches!(
                queue.offer(failed(&sender), 8),
                KickoffNoticeOffer::Closed(_)
            ),
            "a cancelled delivery's queue takes no further notice"
        );

        let sender = RecordingSender::new();
        let queue = KickoffNoticeQueue::starting_with(starting(&sender));
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        tokio::time::timeout(
            Duration::from_secs(5),
            deliver_kickoff_notices(&MobId::from("lane"), &queue, cancel),
        )
        .await
        .expect("a delivery cancelled before its first send ends at once");
        assert!(
            sender.started().is_empty(),
            "a delivery cancelled before its first send sends nothing"
        );
    }
}
