//! Thread-safe inbox for Meerkat comms.
//!
//! The inbox collects incoming peer and external-event traffic,
//! allowing the agent loop to drain them at turn boundaries.
//!
//! Includes a `Notify` mechanism to wake waiting tasks when new messages arrive.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use parking_lot::{Mutex, RwLock};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, mpsc};

use crate::classify::IngressClassificationContext;
use crate::classify::PreparedIngressItem;
use crate::peer_types::PeerIngressState;
use crate::trust::TrustedPeers;
use crate::types::{Envelope, InboxItem};
use meerkat_core::{
    InteractionId, PeerIngressAdmissionDiagnostic, PeerIngressAuthDecision,
    PeerIngressDiagnosticDisplay, PeerIngressEntrySnapshot, PeerIngressFact, PeerIngressKind,
    PeerIngressQueueSnapshot, PeerInputClass, TerminalityClass,
};

const DEFAULT_INBOX_CAPACITY: usize = 1024;

/// Reason an ingress item was dropped before it reached the classified queue.
///
/// Every variant is a semantic failure mode — the envelope or event did not
/// reach the agent. Silent `Ok(())` used to mask these; now they are typed,
/// logged, and counted so tests can pin down "zero drops on the happy path".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DropReason {
    /// `require_peer_auth` is on, the sender is not in the trusted set, and
    /// the envelope is not auth-exempt (e.g. supervisor-bridge bootstrap).
    UntrustedSender,
    /// Classification rejected the item (e.g. pre-trust policy drop in the
    /// classification context before it ever reached the admission gate).
    ClassificationRejected,
    /// The classified queue is closed (receiver dropped).
    SessionClosed,
    /// The classified queue is at capacity.
    InboxFull,
}

/// Outcome of an admission attempt against the classified inbox.
///
/// Marked `#[must_use]` so that silently discarding a `Dropped` branch at a
/// call site is a compile error — a drop is never "just success".
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdmissionOutcome {
    Admitted,
    Dropped { reason: DropReason },
}

impl AdmissionOutcome {
    /// True iff the item was admitted onto the queue.
    pub fn is_admitted(&self) -> bool {
        matches!(self, AdmissionOutcome::Admitted)
    }

    /// Convert this outcome into a `Result` with the drop reason as the error.
    /// Useful at seams that already flow errors through `?`.
    pub fn into_result(self) -> Result<(), DropReason> {
        match self {
            AdmissionOutcome::Admitted => Ok(()),
            AdmissionOutcome::Dropped { reason } => Err(reason),
        }
    }
}

impl From<DropReason> for meerkat_core::comms::AdmissionDropReason {
    fn from(reason: DropReason) -> Self {
        match reason {
            DropReason::UntrustedSender => {
                meerkat_core::comms::AdmissionDropReason::UntrustedSender
            }
            DropReason::ClassificationRejected => {
                meerkat_core::comms::AdmissionDropReason::ClassificationRejected
            }
            DropReason::SessionClosed => meerkat_core::comms::AdmissionDropReason::SessionClosed,
            DropReason::InboxFull => meerkat_core::comms::AdmissionDropReason::InboxFull,
        }
    }
}

/// A classified inbox entry, pairing an item with its ingress classification.
#[derive(Debug)]
pub(crate) struct ClassifiedInboxEntry {
    pub(crate) raw_item_id: InteractionId,
    pub(crate) item: InboxItem,
    pub(crate) class: PeerInputClass,
    pub(crate) auth: PeerIngressAuthDecision,
    pub(crate) kind: PeerIngressKind,
    pub(crate) from_peer: Option<String>,
    pub(crate) ingress_fact: PeerIngressFact,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) request_id: Option<InteractionId>,
    pub(crate) admission_diagnostic: Option<PeerIngressAdmissionDiagnostic>,
    pub(crate) response_terminality: Option<TerminalityClass>,
    pub(crate) text_projection: String,
}

/// Internal admission decision paired with the peer-trust diagnostic that the
/// enqueue site still needs to stamp on the entry. `AdmissionOutcome` is the
/// public face; the trust copy is diagnostic and never a live trust oracle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AdmissionDecision {
    pub(crate) outcome: AdmissionOutcome,
    pub(crate) admission_diagnostic: Option<PeerIngressAdmissionDiagnostic>,
}

#[derive(Debug)]
struct ClassifiedInboxQueue {
    capacity: usize,
    closed: bool,
    entries: VecDeque<ClassifiedInboxEntry>,
    auth_required: bool,
    /// Shared canonical trust set, owned by the comms `Router`.
    ///
    /// The inbox does NOT mirror trust into a local cache — it consults the
    /// router's single source of truth. The same `Arc<RwLock<TrustedPeers>>`
    /// is used at the classify stage (`IngressClassificationContext`) and
    /// at the admission stage under this queue's `Mutex`. Admission re-reads
    /// the trust set atomically with the enqueue so a revoke between
    /// classification (T0) and admission (T2) cannot admit an envelope
    /// that was no longer trusted at T2 — see C-H3.
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    phase: PeerIngressState,
    dropped_count: Arc<AtomicU64>,
}

impl ClassifiedInboxQueue {
    fn new(capacity: usize, auth_required: bool, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        Self {
            capacity,
            closed: false,
            entries: VecDeque::new(),
            auth_required,
            trusted_peers,
            phase: PeerIngressState::Absent,
            dropped_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.dropped_count.clone()
    }

    /// Admit a prepared item into the peer-ingress lifecycle.
    ///
    /// Returns a typed `AdmissionDecision`:
    /// - Plain events are always admitted, with no trust diagnostic.
    /// - External envelopes consult the canonical trust set
    ///   (`Arc<RwLock<TrustedPeers>>` shared with the router) plus the
    ///   auth-required policy. Untrusted senders with auth required are
    ///   `Dropped { reason: UntrustedSender }`; the trust diagnostic still
    ///   records the decision for queue visibility.
    ///
    /// C-H3 — the trust check is re-read from the canonical
    /// `trusted_peers` store **inside** this queue's `Mutex` scope, not
    /// inherited from `prepared.trusted_sender`. The prepared bool is
    /// computed at classification time (T0); a trust revoke between T0
    /// and this admission point (T2) must be observed, and it is — a
    /// revoked sender is dropped here even when the classification
    /// observation said trusted. The prepared bool is retained for
    /// diagnostics only (TOCTOU observability).
    ///
    /// Auth-exempt items (bridge bootstrap / idempotency-ack) bypass the
    /// peer-trust gate unconditionally so bootstrapping can complete
    /// before trust edges exist. The typed auth decision flows from
    /// the prepared item so this single admission seam can distinguish
    /// "admit as trusted" from "admit as exempt" — no parallel outer
    /// branch, no classification-time bool that can drift.
    ///
    /// Updates `phase` to track the observable lifecycle:
    /// `Absent → Received` (admitted), `Received → Received` (more work),
    /// `Absent → Dropped` (untrusted with empty queue), untrusted with queued
    /// work keeps `Received`.
    fn admit_peer_receive(&mut self, prepared: &PreparedIngressItem) -> AdmissionDecision {
        let InboxItem::External { envelope } = &prepared.item else {
            return AdmissionDecision {
                outcome: AdmissionOutcome::Admitted,
                admission_diagnostic: None,
            };
        };
        // Authoritative trust check at admission time. The read lock on
        // `trusted_peers` runs while we hold the queue `Mutex`, so any
        // concurrent `router.{add,remove}_trusted_peer` serializes with
        // this check — classification's stale view is ignored.
        let trusted = self.trusted_peers.read().is_trusted(&envelope.from);
        // Auth-exempt items always admit; otherwise admission is gated by
        // the authoritative trust read against the require_peer_auth
        // policy. The prepared classification observation is NOT consulted
        // here — only the admission-time read is authoritative.
        let admitted = prepared.auth.is_exempt() || trusted || !self.auth_required;
        let had_queued_work = !self.entries.is_empty();

        if admitted {
            self.phase = PeerIngressState::Received;
            AdmissionDecision {
                outcome: AdmissionOutcome::Admitted,
                admission_diagnostic: Some(PeerIngressAdmissionDiagnostic::from_trusted(trusted)),
            }
        } else {
            if had_queued_work {
                self.phase = PeerIngressState::Received;
            } else {
                self.phase = PeerIngressState::Dropped;
            }
            AdmissionDecision {
                outcome: AdmissionOutcome::Dropped {
                    reason: DropReason::UntrustedSender,
                },
                admission_diagnostic: Some(PeerIngressAdmissionDiagnostic::UntrustedAtAdmission),
            }
        }
    }

    /// Advance `phase` on dequeue.
    ///
    /// External items drive the transition: an empty queue after dequeue moves
    /// to `Delivered`; otherwise we stay in `Received`. Plain events do not
    /// drive the peer-ingress phase. Auth-exempt entries (bridge bootstrap
    /// / idempotency-ack envelopes) bypass the phase machinery entirely.
    fn note_peer_dequeue(&mut self, entry: &ClassifiedInboxEntry) {
        if entry.auth.is_exempt() {
            return;
        }
        let InboxItem::External { .. } = &entry.item else {
            return;
        };
        if self.entries.is_empty() {
            self.phase = PeerIngressState::Delivered;
        } else {
            self.phase = PeerIngressState::Received;
        }
    }

    fn try_push(&mut self, entry: ClassifiedInboxEntry) -> Result<(), InboxError> {
        if self.closed {
            return Err(InboxError::Closed);
        }
        if self.entries.len() >= self.capacity {
            return Err(InboxError::Full);
        }
        self.entries.push_back(entry);
        Ok(())
    }

    fn drain(&mut self) -> Vec<ClassifiedInboxEntry> {
        let drained: Vec<_> = self.entries.drain(..).collect();
        for entry in &drained {
            self.note_peer_dequeue(entry);
        }
        drained
    }

    fn pop_front(&mut self) -> Option<ClassifiedInboxEntry> {
        let entry = self.entries.pop_front();
        if let Some(entry_ref) = entry.as_ref() {
            self.note_peer_dequeue(entry_ref);
        }
        entry
    }

    fn close(&mut self) {
        self.closed = true;
    }

    fn snapshot(&self) -> PeerIngressQueueSnapshot {
        let queued_entries: Vec<_> = self.entries.iter().map(snapshot_entry).collect();
        let actionable_count = queued_entries
            .iter()
            .filter(|entry| entry.class.is_actionable())
            .count();
        let response_count = queued_entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.class,
                    PeerInputClass::ResponseProgress | PeerInputClass::ResponseTerminal
                )
            })
            .count();
        let lifecycle_count = queued_entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.class,
                    PeerInputClass::PeerLifecycleAdded
                        | PeerInputClass::PeerLifecycleRetired
                        | PeerInputClass::PeerLifecycleUnwired
                )
            })
            .count();
        let silent_request_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::SilentRequest)
            .count();
        let ack_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::Ack)
            .count();
        let plain_event_count = queued_entries
            .iter()
            .filter(|entry| entry.class == PeerInputClass::PlainEvent)
            .count();

        PeerIngressQueueSnapshot {
            total_count: queued_entries.len(),
            actionable_count,
            response_count,
            lifecycle_count,
            silent_request_count,
            ack_count,
            plain_event_count,
            queued_entries,
        }
    }

    fn runtime_snapshot(&self) -> (PeerIngressQueueSnapshot, PeerIngressState, usize) {
        let queue_len = self.entries.len();
        (self.snapshot(), self.phase, queue_len)
    }
}

/// The receiving end of the inbox, held by the agent loop.
pub struct Inbox {
    rx: mpsc::Receiver<InboxItem>,
    /// Notifier to wake waiting tasks when messages arrive.
    notify: Arc<Notify>,
    /// Classified entries queue (parallel to raw channel).
    classified_queue: Option<Arc<Mutex<ClassifiedInboxQueue>>>,
    /// Notifier that fires only for actionable inputs.
    actionable_notify: Option<Arc<Notify>>,
    /// Shared drop counter (cloned into `InboxSender`). Classified path only.
    dropped_count: Option<Arc<AtomicU64>>,
}

/// The sending end of the inbox, cloned to IO tasks.
#[derive(Clone)]
pub struct InboxSender {
    tx: mpsc::Sender<InboxItem>,
    /// Notifier to wake waiting tasks when messages arrive.
    notify: Arc<Notify>,
    /// Classification context (None for non-classified path).
    classification_context: Option<Arc<IngressClassificationContext>>,
    /// Classified entries queue (parallel to raw channel).
    classified_queue: Option<Arc<Mutex<ClassifiedInboxQueue>>>,
    /// Notifier that fires only for actionable inputs.
    actionable_notify: Option<Arc<Notify>>,
    /// Shared drop counter. Classified path only.
    dropped_count: Option<Arc<AtomicU64>>,
}

impl Inbox {
    /// Create a new inbox, returning both the inbox and a sender.
    pub fn new() -> (Self, InboxSender) {
        Self::new_with_capacity(DEFAULT_INBOX_CAPACITY)
    }

    /// Create a new inbox with a bounded capacity.
    pub fn new_with_capacity(capacity: usize) -> (Self, InboxSender) {
        let (tx, rx) = mpsc::channel(capacity);
        let notify = Arc::new(Notify::new());
        (
            Inbox {
                rx,
                notify: notify.clone(),
                classified_queue: None,
                actionable_notify: None,
                dropped_count: None,
            },
            InboxSender {
                tx,
                notify,
                classification_context: None,
                classified_queue: None,
                actionable_notify: None,
                dropped_count: None,
            },
        )
    }

    /// Create a new inbox with ingress classification support.
    ///
    /// The classified path uses its own owned queue.
    /// `send_classified()` enqueues only on that classified queue.
    /// The raw `tx`/`rx` are kept for structural compatibility with `InboxSender::send()`
    /// but are not written to on the classified path (0.4.10 vestige; can be removed
    /// once `tx`/`rx` are made `Option` in a minor release).
    pub(crate) fn new_classified(
        context: Arc<IngressClassificationContext>,
    ) -> (Self, InboxSender) {
        let (tx, rx) = mpsc::channel(DEFAULT_INBOX_CAPACITY);
        let notify = Arc::new(Notify::new());
        let actionable_notify = Arc::new(Notify::new());
        let queue = ClassifiedInboxQueue::new(
            DEFAULT_INBOX_CAPACITY,
            context.require_peer_auth,
            context.trusted_peers.clone(),
        );
        let dropped_count = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        (
            Inbox {
                rx,
                notify: notify.clone(),
                classified_queue: Some(classified_queue.clone()),
                actionable_notify: Some(actionable_notify.clone()),
                dropped_count: Some(dropped_count.clone()),
            },
            InboxSender {
                tx,
                notify,
                classification_context: Some(context),
                classified_queue: Some(classified_queue),
                actionable_notify: Some(actionable_notify),
                dropped_count: Some(dropped_count),
            },
        )
    }

    /// Get the notifier for waiting on new messages.
    ///
    /// Use this to wake a task when a message arrives, enabling
    /// interrupt-based wait patterns.
    pub fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Receive the next item from the inbox, blocking until one is available.
    pub async fn recv(&mut self) -> Option<InboxItem> {
        self.rx.recv().await
    }

    /// Try to drain all currently available items without blocking.
    pub fn try_drain(&mut self) -> Vec<InboxItem> {
        let mut items = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            items.push(item);
        }
        items
    }

    /// Try to drain all classified entries without blocking.
    pub(crate) fn try_drain_classified(&mut self) -> Vec<ClassifiedInboxEntry> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().drain())
            .unwrap_or_default()
    }

    /// Try to receive a single classified entry without blocking.
    pub(crate) fn try_recv_one_classified(&mut self) -> Option<ClassifiedInboxEntry> {
        self.classified_queue.as_ref()?.lock().pop_front()
    }

    /// Get the actionable-input notifier (fires only for actionable classes).
    pub(crate) fn classified_actionable_notify(&self) -> Option<Arc<Notify>> {
        self.actionable_notify.clone()
    }

    /// Snapshot the classified ingress queue without draining it.
    pub(crate) fn classified_snapshot(&self) -> Option<PeerIngressQueueSnapshot> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().snapshot())
    }

    pub(crate) fn peer_runtime_snapshot(
        &self,
    ) -> Option<(PeerIngressQueueSnapshot, PeerIngressState, usize)> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().runtime_snapshot())
    }

    /// Read the classified-inbox drop counter.
    ///
    /// Every `Dropped` outcome from `admit_peer_receive` (plus any failure
    /// before enqueue — full queue, closed queue, classification rejection)
    /// increments this counter. Tests assert this is zero on clean delivery
    /// scenarios to pin down the "no silent drops" invariant.
    ///
    /// Returns `None` on non-classified inboxes (there is no admission step).
    pub fn dropped_count(&self) -> Option<u64> {
        self.dropped_count
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn peer_authority_test_snapshot(&self) -> Option<(PeerIngressState, usize)> {
        self.classified_queue.as_ref().map(|queue| {
            let queue = queue.lock();
            (queue.phase, queue.entries.len())
        })
    }

    /// Test-only: ask the queue's shared trust set whether `pubkey` is
    /// currently trusted. The queue does not own a local copy — this
    /// reads from the same `Arc<RwLock<TrustedPeers>>` that the router
    /// mutates and that the classifier consults.
    #[cfg(test)]
    pub(crate) fn peer_authority_trusts_peer_for_test(
        &self,
        pubkey: &crate::identity::PubKey,
    ) -> Option<bool> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().trusted_peers.read().is_trusted(pubkey))
    }
}

impl InboxSender {
    /// **Test-only seam for C-H3.** Simulate the classify-then-admit
    /// ordering with an explicit pause between the two steps so a test
    /// can mutate the trust set in the gap.
    ///
    /// Returns `None` on non-classified inboxes (there is no admission
    /// step to exercise). Marked `#[doc(hidden)]` so it is callable from
    /// integration tests without appearing on the public API surface.
    #[doc(hidden)]
    pub fn classified_admit_with_pause_for_test(
        &self,
        item: InboxItem,
        pause: impl FnOnce(),
    ) -> AdmissionOutcome {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            return self.record_drop(DropReason::SessionClosed);
        };
        let result = match ctx.prepare(item) {
            Some(r) => r,
            None => return self.record_drop(DropReason::ClassificationRejected),
        };
        // Classification done — hand control to the caller so it can
        // flip trust state before admission runs. This is the critical
        // T0→T1 window that the pre-C-H3 code would have leaked.
        pause();
        let kind = result.kind;
        let decision = {
            let mut queue = classified_queue.lock();
            queue.admit_peer_receive(&result)
        };
        if let AdmissionOutcome::Dropped { reason } = decision.outcome {
            return self.record_drop(reason);
        }
        let entry = ClassifiedInboxEntry {
            raw_item_id: result.raw_item_id,
            item: result.item,
            class: result.class,
            auth: result.auth,
            kind,
            from_peer: result.from_peer,
            ingress_fact: result.ingress_fact,
            lifecycle_peer: result.lifecycle_peer,
            request_id: result.request_id,
            admission_diagnostic: decision.admission_diagnostic,
            response_terminality: result.response_terminality,
            text_projection: result.text_projection,
        };
        match classified_queue.lock().try_push(entry) {
            Ok(()) => {
                self.notify.notify_waiters();
                AdmissionOutcome::Admitted
            }
            Err(InboxError::Closed) => self.record_drop(DropReason::SessionClosed),
            Err(InboxError::Full) => self.record_drop(DropReason::InboxFull),
        }
    }

    /// Admit one external envelope at the inbox seam.
    ///
    /// This keeps trust ownership inside `InboxSender` instead of the
    /// transport task. Classified runtimes route through `send_classified()`;
    /// raw inboxes enforce the same auth-required admission rule here so
    /// `handle_connection()` does not branch on trust itself.
    pub fn send_connection_ingress(
        &self,
        envelope: Envelope,
        require_peer_auth: bool,
        trusted_peers: &Arc<RwLock<TrustedPeers>>,
    ) -> AdmissionOutcome {
        if self.classification_context.is_some() {
            return self.send_classified(InboxItem::External { envelope });
        }

        // Raw inbox ingress is transport mechanics only. It must not reconstruct
        // auth exemptions, lifecycle intent, or response terminality from the
        // compatibility classifier; those facts exist only on the classified
        // runtime/machine path.
        let auth = PeerIngressAuthDecision::Required;
        if require_peer_auth
            && !auth.is_exempt()
            && !trusted_peers.read().is_trusted(&envelope.from)
        {
            tracing::warn!(
                peer_id = %envelope.from.to_peer_id(),
                auth_required = require_peer_auth,
                auth_exempt = auth.is_exempt(),
                reason = ?DropReason::UntrustedSender,
                "raw inbox dropped external peer ingress at admission"
            );
            return self.record_drop(DropReason::UntrustedSender);
        }

        self.send(InboxItem::External { envelope })
    }

    /// Send an item to the inbox.
    ///
    /// On classified runtimes, delegates to `send_classified()` so the item
    /// goes through the classified queue (the sole consumer). On non-classified
    /// runtimes, enqueues on the raw channel directly.
    ///
    /// Returns a typed `AdmissionOutcome`. Drops are never surfaced as `Ok(())`.
    pub fn send(&self, item: InboxItem) -> AdmissionOutcome {
        // If classification context is available, route through classified path.
        if self.classification_context.is_some() {
            return self.send_classified(item);
        }
        match self.tx.try_send(item) {
            Ok(()) => {
                self.notify.notify_waiters();
                AdmissionOutcome::Admitted
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!(
                    reason = ?DropReason::SessionClosed,
                    "raw inbox dropped item: queue closed"
                );
                self.record_drop(DropReason::SessionClosed)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    reason = ?DropReason::InboxFull,
                    "raw inbox dropped item: queue full"
                );
                self.record_drop(DropReason::InboxFull)
            }
        }
    }

    /// Send an item with classification through the classified queue.
    ///
    /// Returns a typed `AdmissionOutcome` so that silent discards become a
    /// compile error. `Dropped` outcomes carry a `DropReason`, are emitted as
    /// `tracing::warn!`, and increment the queue's drop counter. Tests assert
    /// on the counter to prove the happy path doesn't leak drops.
    ///
    /// Transport/queue-level errors (closed queue, full queue) are surfaced as
    /// `Dropped` outcomes rather than bubbled `Err`s — they're semantic drops
    /// from the caller's perspective, not programmer errors.
    ///
    /// Classified items are enqueued only on the classified queue, then the
    /// appropriate notify is fired.
    pub fn send_classified(&self, item: InboxItem) -> AdmissionOutcome {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            // Non-classified fallback: enqueue on raw and report back as typed
            // outcome. Use try_send directly to avoid infinite recursion with
            // `send()` (which delegates to us when a classification context is
            // present).
            return match self.tx.try_send(item) {
                Ok(()) => {
                    self.notify.notify_waiters();
                    AdmissionOutcome::Admitted
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::warn!(
                        reason = ?DropReason::SessionClosed,
                        "classified inbox send_classified fell back to raw and found queue closed"
                    );
                    self.record_drop(DropReason::SessionClosed)
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        reason = ?DropReason::InboxFull,
                        "classified inbox send_classified fell back to raw and found queue full"
                    );
                    self.record_drop(DropReason::InboxFull)
                }
            };
        };

        let result = match ctx.prepare(item) {
            Some(r) => r,
            None => {
                // Classification rejected the item before admission (e.g.
                // pre-trust policy drop). No envelope context to log.
                tracing::warn!(
                    reason = ?DropReason::ClassificationRejected,
                    "classified inbox dropped item at classification stage"
                );
                return self.record_drop(DropReason::ClassificationRejected);
            }
        };
        let kind = result.kind;
        // C-H3 — the admission decision (trust re-check, auth-exempt
        // bypass, and phase update) happens inside a single queue-lock
        // scope so classification's T0 trust observation cannot admit an
        // envelope that was revoked at T2.
        let decision = {
            let mut queue = classified_queue.lock();
            queue.admit_peer_receive(&result)
        };
        match decision.outcome {
            AdmissionOutcome::Admitted => {}
            AdmissionOutcome::Dropped { reason } => {
                if let InboxItem::External { envelope } = &result.item {
                    let request_id_for_log =
                        result.request_id.map(|request_id| request_id.to_string());
                    tracing::warn!(
                        peer_id = %envelope.from.to_peer_id(),
                        from_peer = result.from_peer.as_deref().unwrap_or("<unresolved>"),
                        class = ?result.class,
                        kind = ?kind,
                        request_id = request_id_for_log.as_deref().unwrap_or("<none>"),
                        auth_required = ctx.require_peer_auth,
                        trusted_sender = result.trusted_sender,
                        reason = ?reason,
                        "classified inbox dropped external peer ingress at admission"
                    );
                }
                return self.record_drop(reason);
            }
        }
        let entry = ClassifiedInboxEntry {
            raw_item_id: result.raw_item_id,
            item: result.item,
            class: result.class,
            auth: result.auth,
            kind,
            from_peer: result.from_peer,
            ingress_fact: result.ingress_fact,
            lifecycle_peer: result.lifecycle_peer,
            request_id: result.request_id,
            admission_diagnostic: decision.admission_diagnostic,
            response_terminality: result.response_terminality,
            text_projection: result.text_projection,
        };
        // Enqueue only on classified queue (no raw double-enqueue).
        // drain_classified_inbox_interactions() is the sole consumer.
        let is_actionable = entry.class.is_actionable();
        let push_result = classified_queue.lock().try_push(entry);
        match push_result {
            Ok(()) => {
                // Fire actionable notify only for actionable classes.
                if is_actionable && let Some(ref actionable) = self.actionable_notify {
                    actionable.notify_waiters();
                }
                // Always fire broad notify.
                self.notify.notify_waiters();
                AdmissionOutcome::Admitted
            }
            Err(InboxError::Closed) => {
                tracing::warn!(
                    kind = ?kind,
                    reason = ?DropReason::SessionClosed,
                    "classified inbox dropped item: queue closed"
                );
                self.record_drop(DropReason::SessionClosed)
            }
            Err(InboxError::Full) => {
                tracing::warn!(
                    kind = ?kind,
                    reason = ?DropReason::InboxFull,
                    "classified inbox dropped item: queue full"
                );
                self.record_drop(DropReason::InboxFull)
            }
        }
    }

    fn record_drop(&self, reason: DropReason) -> AdmissionOutcome {
        if let Some(counter) = &self.dropped_count {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        AdmissionOutcome::Dropped { reason }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        if let Some(queue) = &self.classified_queue {
            queue.lock().close();
        }
    }
}

fn snapshot_entry(entry: &ClassifiedInboxEntry) -> PeerIngressEntrySnapshot {
    PeerIngressEntrySnapshot {
        raw_item_id: entry.raw_item_id,
        interaction_id: match &entry.item {
            InboxItem::External { envelope } => Some(InteractionId(envelope.id)),
            InboxItem::PlainEvent { interaction_id, .. } => interaction_id.map(InteractionId),
        },
        class: entry.class,
        kind: entry.kind,
        from_peer_display: entry
            .from_peer
            .as_ref()
            .map(|peer| PeerIngressDiagnosticDisplay::new(peer.clone())),
        canonical_peer_id: entry.ingress_fact.canonical_peer_id,
        display_name: entry.ingress_fact.display_name.clone(),
        signing_pubkey: entry.ingress_fact.signing_pubkey,
        route: entry.ingress_fact.route.clone(),
        lifecycle_peer_display: entry
            .lifecycle_peer
            .as_ref()
            .map(|peer| PeerIngressDiagnosticDisplay::new(peer.clone())),
        request_correlation_id: entry.request_id,
        auth: match &entry.item {
            InboxItem::External { .. } => Some(entry.auth),
            InboxItem::PlainEvent { .. } => None,
        },
        admission_diagnostic: entry.admission_diagnostic,
        response_terminality: entry.response_terminality,
    }
}

/// Errors that can occur with inbox operations.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    #[error("Inbox has been closed")]
    Closed,
    #[error("Inbox is full")]
    Full,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use crate::types::{Envelope, MessageKind};
    use uuid::Uuid;

    fn make_test_envelope() -> Envelope {
        Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                blocks: None,
                body: "test".to_string(),
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        }
    }

    fn make_request_envelope() -> Envelope {
        Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 42}),
                blocks: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        }
    }

    fn make_response_envelope(in_reply_to: Uuid) -> Envelope {
        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Response {
            in_reply_to,
            status: crate::types::Status::Completed,
            result: serde_json::json!({"ok": true}),
            handling_mode: None,
        };
        envelope
    }

    fn make_lifecycle_envelope(peer: &str) -> Envelope {
        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Lifecycle {
            kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
            params: serde_json::json!({ "peer": peer }),
        };
        envelope
    }

    #[test]
    fn test_inbox_struct() {
        let (inbox, _sender) = Inbox::new();
        // Inbox exists and has the receiver
        drop(inbox);
    }

    #[test]
    fn test_inbox_sender_struct() {
        let (_inbox, sender) = Inbox::new();
        // Sender exists and can be cloned
        let _sender2 = sender;
    }

    #[test]
    fn test_inbox_new() {
        let (inbox, sender) = Inbox::new();
        // Both parts exist
        drop(inbox);
        drop(sender);
    }

    #[tokio::test]
    async fn test_inbox_sender_send() {
        let (_inbox, sender) = Inbox::new();
        let item = InboxItem::External {
            envelope: make_test_envelope(),
        };
        let result = sender.send(item);
        assert!(result.is_admitted());
    }

    #[tokio::test]
    async fn test_inbox_recv() {
        let (mut inbox, sender) = Inbox::new();
        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        sender
            .send(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let received = inbox.recv().await;
        assert!(received.is_some());
        match received.unwrap() {
            InboxItem::External { envelope } => {
                assert_eq!(envelope.id, envelope_id);
            }
            _ => panic!("expected External variant"),
        }
    }

    #[tokio::test]
    async fn test_inbox_try_drain() {
        let (mut inbox, sender) = Inbox::new();

        // Send multiple items
        for i in 0..3 {
            let mut envelope = make_test_envelope();
            envelope.id = Uuid::from_u128(i as u128);
            sender
                .send(InboxItem::External { envelope })
                .into_result()
                .unwrap();
        }

        // Give a moment for items to be queued
        tokio::task::yield_now().await;

        // Drain all at once
        let items = inbox.try_drain();
        assert_eq!(items.len(), 3);

        // Verify IDs
        for (i, item) in items.into_iter().enumerate() {
            match item {
                InboxItem::External { envelope } => {
                    assert_eq!(envelope.id.as_u128(), i as u128);
                }
                _ => panic!("expected External variant"),
            }
        }

        // No more items
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }

    #[test]
    fn test_sender_error_on_closed_inbox() {
        let (inbox, sender) = Inbox::new();
        drop(inbox); // Close the inbox

        let result = sender.send(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert!(matches!(
            result,
            AdmissionOutcome::Dropped {
                reason: DropReason::SessionClosed
            }
        ));
    }

    #[test]
    fn test_classified_sender_error_on_closed_inbox() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let counter = sender.dropped_count.clone();
        drop(inbox);

        let result = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert!(matches!(
            result,
            AdmissionOutcome::Dropped {
                reason: DropReason::SessionClosed
            }
        ));
        // And the drop counter must have ticked — the silent `Ok(())` bug is gone.
        let c = counter.expect("classified path must expose a counter");
        assert_eq!(c.load(Ordering::Relaxed), 1);
    }

    // Phase 3: Wait interruption tests

    #[test]
    fn test_inbox_has_notify() {
        let (inbox, _sender) = Inbox::new();
        // Inbox should expose a Notify
        let notify = inbox.notify();
        // Arc should be clonable
        drop(notify);
    }

    #[tokio::test]
    async fn test_sender_notifies_on_send() {
        let (inbox, sender) = Inbox::new();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let notified = notify.notified();

        // Send a message - this should notify waiters
        sender
            .send(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        // The notification should complete immediately (message was sent before we awaited)
        // Use timeout to ensure we don't hang if notify doesn't work
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;

        assert!(result.is_ok(), "Should have been notified after send");
    }

    #[tokio::test]
    async fn test_notify_wakes_waiting_task() {
        let (inbox, sender) = Inbox::new();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let handle = tokio::spawn(async move {
            notify.notified().await;
            "woken"
        });

        // Give the task time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Send a message - this should wake the waiting task
        sender
            .send(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        // The task should complete
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), handle).await;

        assert!(result.is_ok(), "Task should have completed");
        assert_eq!(result.unwrap().unwrap(), "woken");
    }

    // === Classified inbox tests ===

    use crate::classify::IngressClassificationContext;
    use crate::trust::{TrustedPeer, TrustedPeers};
    use meerkat_core::PeerIngressMachinePolicy;
    use std::sync::atomic::AtomicUsize;

    fn make_classification_context(
        trusted: TrustedPeers,
        require_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        make_classification_context_with_machine(trusted, require_auth, None)
    }

    fn make_classification_context_with_machine(
        trusted: TrustedPeers,
        require_auth: bool,
        peer_comms_handle: Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>>,
    ) -> Arc<IngressClassificationContext> {
        Arc::new(IngressClassificationContext {
            require_peer_auth: require_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted)),
            ingress_policy: Arc::new(PeerIngressMachinePolicy::default()),
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(peer_comms_handle)),
            require_machine_authority: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            inproc_namespace: None,
        })
    }

    fn make_trusted(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "inproc://test".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }
    }

    struct RejectingPeerCommsHandle {
        external_calls: AtomicUsize,
    }

    impl RejectingPeerCommsHandle {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
            })
        }

        fn external_calls(&self) -> usize {
            self.external_calls.load(Ordering::SeqCst)
        }
    }

    impl meerkat_core::handles::PeerCommsHandle for RejectingPeerCommsHandle {
        fn classify_external_envelope(
            &self,
            _facts: meerkat_core::PeerIngressEnvelopeFacts,
        ) -> Result<meerkat_core::PeerIngressAdmission, meerkat_core::handles::DslTransitionError>
        {
            self.external_calls.fetch_add(1, Ordering::SeqCst);
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "test_peer_comms::classify_external_envelope",
                "machine rejected external ingress",
            ))
        }

        fn classify_plain_event(
            &self,
            facts: meerkat_core::PeerIngressPlainEventFacts,
        ) -> Result<meerkat_core::PeerIngressAdmission, meerkat_core::handles::DslTransitionError>
        {
            Ok(PeerIngressMachinePolicy::default().classify_plain_event_facts(&facts))
        }

        fn set_peer_ingress_context(
            &self,
            _keep_alive: bool,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_classified_inbox_actionable_notify_fires_for_message() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();
        let notified = actionable.notified();

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;
        assert!(result.is_ok(), "Actionable notify should fire for messages");
    }

    #[tokio::test]
    async fn test_classified_inbox_actionable_notify_fires_for_request() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();
        let notified = actionable.notified();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Request {
            intent: "review".to_string(),
            params: serde_json::json!({}),
            blocks: None,
            handling_mode: None,
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;
        assert!(result.is_ok(), "Actionable notify should fire for requests");
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_response() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Response {
            in_reply_to: Uuid::new_v4(),
            status: crate::types::Status::Completed,
            result: serde_json::json!({}),
            handling_mode: None,
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for responses"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_ack() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Ack {
            in_reply_to: Uuid::new_v4(),
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for acks"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_plain_event() {
        let ctx = make_classification_context(TrustedPeers::new(), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        sender
            .send_classified(InboxItem::PlainEvent {
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for plain events"
        );
    }

    #[tokio::test]
    async fn test_classified_inbox_no_actionable_notify_for_lifecycle() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Request {
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({"peer": "new-agent"}),
            blocks: None,
            handling_mode: None,
        };

        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let notified = actionable.notified();
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;
        assert!(
            result.is_err(),
            "Actionable notify should NOT fire for lifecycle events"
        );
    }

    #[tokio::test]
    async fn test_classified_try_drain() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        let entries = inbox.try_drain_classified();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].raw_item_id.to_string().is_empty());
        assert_eq!(
            entries[0].class,
            meerkat_core::PeerInputClass::ActionableMessage
        );
    }

    #[tokio::test]
    async fn test_classified_send_does_not_populate_raw_channel() {
        // Classified send only enqueues on the classified channel.
        // No raw double-enqueue; drain_classified_inbox_interactions() is the sole consumer.
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        // Raw channel should be empty
        let raw = inbox.try_drain();
        assert_eq!(
            raw.len(),
            0,
            "classified send should not double-enqueue to raw channel"
        );
        // Classified channel should have the item
        let classified = inbox.try_drain_classified();
        assert_eq!(classified.len(), 1);
    }

    #[tokio::test]
    async fn test_classified_snapshot_is_non_destructive() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();
        sender
            .send_classified(InboxItem::PlainEvent {
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.actionable_count, 2);
        assert_eq!(snapshot.plain_event_count, 1);
        assert_eq!(snapshot.queued_entries.len(), 2);
        assert_eq!(snapshot.queued_entries[0].kind, PeerIngressKind::Message);
        assert_eq!(snapshot.queued_entries[1].kind, PeerIngressKind::PlainEvent);
        assert_eq!(
            snapshot.queued_entries[0].auth,
            Some(PeerIngressAuthDecision::Required)
        );
        assert_eq!(snapshot.queued_entries[1].auth, None);
        assert_eq!(
            snapshot.queued_entries[0].admission_diagnostic,
            Some(PeerIngressAdmissionDiagnostic::TrustedAtAdmission)
        );
        assert_eq!(snapshot.queued_entries[1].admission_diagnostic, None);
        assert!(
            !snapshot.queued_entries[0]
                .raw_item_id
                .to_string()
                .is_empty(),
            "external items should keep a stable ingress raw item id"
        );
        assert!(
            snapshot.queued_entries[1].interaction_id.is_some(),
            "plain events without an explicit interaction id should be assigned one at ingress"
        );
        assert_eq!(
            snapshot.queued_entries[1].raw_item_id,
            snapshot.queued_entries[1]
                .interaction_id
                .expect("plain event interaction id should be present")
        );

        let drained = inbox.try_drain_classified();
        assert_eq!(drained.len(), 2, "snapshot must not drain queued ingress");
    }

    #[tokio::test]
    async fn test_classified_snapshot_preserves_request_correlation() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let envelope = make_request_envelope();
        let request_id = InteractionId(envelope.id);

        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.total_count, 1);
        assert_eq!(snapshot.queued_entries.len(), 1);
        assert_eq!(snapshot.queued_entries[0].kind, PeerIngressKind::Request);
        assert_eq!(
            snapshot.queued_entries[0].request_correlation_id,
            Some(request_id)
        );
        assert_eq!(
            snapshot.queued_entries[0].admission_diagnostic,
            Some(PeerIngressAdmissionDiagnostic::TrustedAtAdmission)
        );
    }

    #[tokio::test]
    async fn test_classified_snapshot_diagnostics_cover_admitted_shapes() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let message = make_test_envelope();
        let message_id = InteractionId(message.id);
        let request = make_request_envelope();
        let request_id = InteractionId(request.id);
        let response_reply_to = Uuid::new_v4();
        let response = make_response_envelope(response_reply_to);
        let lifecycle = make_lifecycle_envelope("worker-1");

        for envelope in [message, request, response, lifecycle] {
            sender
                .send_classified(InboxItem::External { envelope })
                .into_result()
                .unwrap();
        }
        sender
            .send_classified(InboxItem::PlainEvent {
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.total_count, 5);
        assert_eq!(snapshot.actionable_count, 4);
        assert_eq!(snapshot.response_count, 1);
        assert_eq!(snapshot.lifecycle_count, 1);
        assert_eq!(snapshot.plain_event_count, 1);
        assert_eq!(snapshot.ack_count, 0);
        assert_eq!(snapshot.silent_request_count, 0);

        let entries = &snapshot.queued_entries;
        assert_eq!(entries[0].kind, PeerIngressKind::Message);
        assert_eq!(entries[0].raw_item_id, message_id);
        assert_eq!(entries[0].interaction_id, Some(message_id));
        assert_eq!(
            entries[0]
                .from_peer_display
                .as_ref()
                .map(meerkat_core::PeerIngressDiagnosticDisplay::as_str),
            Some("peer")
        );
        assert_eq!(entries[0].lifecycle_peer_display, None);
        assert_eq!(entries[0].request_correlation_id, None);
        assert_eq!(
            entries[0].admission_diagnostic,
            Some(PeerIngressAdmissionDiagnostic::TrustedAtAdmission)
        );

        assert_eq!(entries[1].kind, PeerIngressKind::Request);
        assert_eq!(entries[1].request_correlation_id, Some(request_id));
        assert_eq!(
            entries[1]
                .from_peer_display
                .as_ref()
                .map(meerkat_core::PeerIngressDiagnosticDisplay::as_str),
            Some("peer")
        );

        assert_eq!(entries[2].kind, PeerIngressKind::Response);
        assert_eq!(
            entries[2].request_correlation_id,
            Some(InteractionId(response_reply_to))
        );
        assert_eq!(
            entries[2]
                .from_peer_display
                .as_ref()
                .map(meerkat_core::PeerIngressDiagnosticDisplay::as_str),
            Some("peer")
        );

        assert_eq!(entries[3].kind, PeerIngressKind::Request);
        assert_eq!(entries[3].class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(
            entries[3]
                .lifecycle_peer_display
                .as_ref()
                .map(meerkat_core::PeerIngressDiagnosticDisplay::as_str),
            Some("worker-1")
        );
        assert_eq!(
            entries[3]
                .from_peer_display
                .as_ref()
                .map(meerkat_core::PeerIngressDiagnosticDisplay::as_str),
            Some("peer")
        );

        assert_eq!(entries[4].kind, PeerIngressKind::PlainEvent);
        assert_eq!(entries[4].from_peer_display, None);
        assert_eq!(entries[4].lifecycle_peer_display, None);
        assert_eq!(entries[4].request_correlation_id, None);
        assert_eq!(entries[4].auth, None);
        assert_eq!(entries[4].admission_diagnostic, None);
        assert_eq!(entries[4].raw_item_id, entries[4].interaction_id.unwrap());
    }

    #[tokio::test]
    async fn test_classified_snapshot_trust_observation_is_diagnostic_only() {
        let ctx = make_classification_context(TrustedPeers::new(), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        let snapshot = inbox
            .classified_snapshot()
            .expect("classified snapshot should exist");
        assert_eq!(snapshot.queued_entries.len(), 1);
        assert_eq!(
            snapshot.queued_entries[0].admission_diagnostic,
            Some(PeerIngressAdmissionDiagnostic::UntrustedAtAdmission),
            "the queued row records an observation, not an admission oracle"
        );
        assert!(
            snapshot.queued_entries[0].from_peer_display.is_some(),
            "the fallback sender label is display-only diagnostics"
        );

        let drained = inbox.try_drain_classified();
        assert_eq!(
            drained.len(),
            1,
            "admission was governed by policy, not reconstructed from the snapshot"
        );
    }

    #[tokio::test]
    async fn test_classified_happy_path_reports_zero_drops() {
        // Acceptance criterion: on a clean delivery scenario the drop counter
        // stays at zero. This pins down the "no silent drops" invariant —
        // regressions that reintroduce `Ok(())`-on-drop would bump it.
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), true);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let outcome = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert_eq!(outcome, AdmissionOutcome::Admitted);
        assert_eq!(
            inbox.dropped_count(),
            Some(0),
            "happy-path delivery must not touch the drop counter"
        );
    }

    #[tokio::test]
    async fn test_classified_untrusted_sender_returns_typed_drop_reason() {
        // Untrusted sender + require_peer_auth: drop is typed, logged, and
        // counted. No more `Ok(())` masquerading as delivery.
        let untrusted_pubkey = PubKey::new([9u8; 32]);
        let ctx = make_classification_context(TrustedPeers::new(), true);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let mut envelope = make_test_envelope();
        envelope.from = untrusted_pubkey;

        let outcome = sender.send_classified(InboxItem::External { envelope });
        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::UntrustedSender
            }
        );
        assert_eq!(inbox.dropped_count(), Some(1));
    }

    #[tokio::test]
    async fn test_machine_rejection_returns_classification_rejected_drop_reason() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let machine = RejectingPeerCommsHandle::new();
        let ctx = make_classification_context_with_machine(
            make_trusted("peer", &sender_pubkey),
            true,
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        let outcome = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });

        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            }
        );
        assert_eq!(inbox.dropped_count(), Some(1));
        assert_eq!(inbox.try_drain_classified().len(), 0);
        assert_eq!(machine.external_calls(), 1);
    }

    #[tokio::test]
    async fn test_runtime_required_response_without_machine_authority_fails_closed() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), true);
        ctx.require_machine_authority.store(true, Ordering::SeqCst);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        let outcome = sender.send_classified(InboxItem::External {
            envelope: make_response_envelope(Uuid::new_v4()),
        });

        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            },
            "runtime-backed peer response must not be accepted or terminalized by the compatibility classifier"
        );
        assert_eq!(inbox.dropped_count(), Some(1));
        assert_eq!(inbox.try_drain_classified().len(), 0);
    }

    #[tokio::test]
    async fn test_classified_full_queue_returns_typed_drop_reason() {
        // InboxFull drops surface as typed `Dropped { InboxFull }` with the
        // counter bumped. We build a classified queue at capacity 1 by using
        // `new_with_capacity` plus then falling through to the classified
        // path with trusted sender: admit the first, reject the second.
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        // `new_classified` uses DEFAULT_INBOX_CAPACITY; to exercise full we
        // need a direct construction path. Fall back to building the queue
        // directly and then driving it.
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = ClassifiedInboxQueue::new(1, ctx.require_peer_auth, ctx.trusted_peers.clone());
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let (tx, _rx) = mpsc::channel::<InboxItem>(1);
        let sender = InboxSender {
            tx,
            notify,
            classification_context: Some(ctx),
            classified_queue: Some(classified_queue),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        let first = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert_eq!(first, AdmissionOutcome::Admitted);
        assert_eq!(counter.load(Ordering::Relaxed), 0);

        let second = sender.send_classified(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert_eq!(
            second,
            AdmissionOutcome::Dropped {
                reason: DropReason::InboxFull
            }
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_raw_connection_ingress_rejects_untrusted_bridge_without_machine_authority() {
        // Raw transport ingress is not a classification authority. Without the
        // classified runtime/machine path, it cannot infer supervisor-bridge
        // auth exemption from a local compatibility policy.
        let receiver = crate::identity::Keypair::generate();
        let sender = crate::identity::Keypair::generate();
        let trusted = Arc::new(parking_lot::RwLock::new(TrustedPeers::new()));
        let (mut inbox, inbox_sender) = Inbox::new();

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender.public_key(),
            to: receiver.public_key(),
            kind: MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        envelope.sign(&sender);

        let outcome = inbox_sender.send_connection_ingress(envelope, true, &trusted);
        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::UntrustedSender
            }
        );

        let items = inbox.try_drain();
        assert!(items.is_empty(), "raw ingress must fail closed");
    }
}
