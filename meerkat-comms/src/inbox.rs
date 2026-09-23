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
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, oneshot};

use crate::classify::{IngressClassificationContext, PeerCommsHandleSlot, PreparedIngressItem};
use crate::peer_types::PeerIngressState;
use crate::trust::TrustStore;
#[cfg(not(target_arch = "wasm32"))]
use crate::types::Envelope;
use crate::types::InboxItem;
use meerkat_core::time_compat::Instant;
use meerkat_core::{
    InteractionId, PeerIngressAdmissionDiagnostic, PeerIngressAuthDecision,
    PeerIngressDeliveryContract, PeerIngressDequeueAuthority, PeerIngressDequeueFacts,
    PeerIngressDiagnosticDisplay, PeerIngressEntrySnapshot, PeerIngressFact, PeerIngressKind,
    PeerIngressQueueSnapshot, PeerIngressReceiveAuthority, PeerIngressReceiveFacts,
    PeerIngressReceiveOutcome, PeerInputClass, TerminalityClass,
};

const DEFAULT_INBOX_CAPACITY: usize = 1024;

#[cfg(test)]
tokio::task_local! {
    static CLASSIFIED_SEND_WAIT_BEFORE_AWAIT_HOOK: Arc<dyn Fn() + Send + Sync>;
}

#[cfg(test)]
fn run_classified_send_wait_before_await_hook() {
    let _ = CLASSIFIED_SEND_WAIT_BEFORE_AWAIT_HOOK.try_with(|hook| hook());
}

#[cfg(not(test))]
fn run_classified_send_wait_before_await_hook() {}

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
#[derive(Debug, Clone)]
pub(crate) struct ClassifiedInboxEntry {
    pub(crate) raw_item_id: InteractionId,
    pub(crate) item: InboxItem,
    pub(crate) class: PeerInputClass,
    /// Machine-owned actionable grouping verdict, mirrored from the
    /// MeerkatMachine PeerIngress classification effect (via `PreparedIngressItem`).
    pub(crate) actionable: bool,
    pub(crate) auth: PeerIngressAuthDecision,
    pub(crate) delivery_contract: PeerIngressDeliveryContract,
    pub(crate) kind: PeerIngressKind,
    pub(crate) from_peer: Option<String>,
    pub(crate) ingress_fact: PeerIngressFact,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) request_id: Option<InteractionId>,
    pub(crate) admission_diagnostic: Option<PeerIngressAdmissionDiagnostic>,
    pub(crate) response_terminality: Option<TerminalityClass>,
    ingress_completion: Option<IngressCompletion>,
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

#[derive(Debug, Clone, Copy)]
struct AdmissionPushDecision {
    outcome: AdmissionOutcome,
    is_actionable: bool,
}

struct ClassifiedInboxQueue {
    capacity: usize,
    closed: bool,
    entries: VecDeque<ClassifiedInboxEntry>,
    auth_required: bool,
    /// Shared canonical trust store, owned by the comms `Router`.
    ///
    /// The inbox does NOT mirror trust into a local cache — it consults the
    /// router's single source of truth. The same `Arc<RwLock<TrustStore>>`
    /// is used at the classify stage (`IngressClassificationContext`) and
    /// at the admission stage under this queue's `Mutex`. Admission re-reads
    /// the trust store atomically with the enqueue so a revoke between
    /// classification (T0) and admission (T2) cannot admit an envelope
    /// that was no longer trusted at T2 — see C-H3.
    trusted_peers: Arc<RwLock<TrustStore>>,
    peer_comms_handle: PeerCommsHandleSlot,
    phase: PeerIngressState,
    dropped_count: Arc<AtomicU64>,
    capacity_notify: Arc<Notify>,
    ingress_notify: Arc<Notify>,
    outstanding_claim: Option<OutstandingClaim>,
    claim_acquired_count: u64,
    handed_off_count: u64,
    durably_admitted_count: u64,
    runtime_handover_count: u64,
    terminal_outcomes: meerkat_core::interaction::PeerIngressTerminalOutcomeCounts,
    last_delivery_correlation: Option<meerkat_core::interaction::PeerIngressDeliveryCorrelation>,
}

#[derive(Debug, Clone)]
struct IngressCompletion(Arc<Mutex<Option<oneshot::Sender<IngressDeliveryOutcome>>>>);

impl IngressCompletion {
    fn new() -> (Self, IngressDeliveryReceipt) {
        let (sender, receiver) = oneshot::channel();
        (
            Self(Arc::new(Mutex::new(Some(sender)))),
            IngressDeliveryReceipt(receiver),
        )
    }

    fn resolve(&self, outcome: IngressDeliveryOutcome) {
        if let Some(sender) = self.0.lock().take() {
            let _ = sender.send(outcome);
        }
    }
}

pub(crate) struct IngressDeliveryReceipt(oneshot::Receiver<IngressDeliveryOutcome>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngressDeliveryOutcome {
    Queued,
    DurablyResolved(meerkat_core::interaction::PeerIngressTerminalOutcomeKind),
    VolatileHandedOff,
    Dropped { reason: DropReason },
}

impl IngressDeliveryReceipt {
    pub(crate) async fn wait(self) -> IngressDeliveryOutcome {
        match self.0.await {
            Ok(outcome) => outcome,
            Err(_) => IngressDeliveryOutcome::Dropped {
                reason: DropReason::SessionClosed,
            },
        }
    }
}

/// Result of a BOUNDED wait for authoritative in-process delivery.
///
/// [`InboxSender::send_wait_for_delivery`] parks the sender twice: once for
/// queue capacity and once for the receiver's ingress authority to commit the
/// exact claim. Both parks are resolved only by the RECEIVER's drain, so both
/// are bounded by a caller-supplied deadline. The three variants keep the
/// three facts apart, because they are not interchangeable:
///
/// * `Resolved` — the receiver's authority produced a terminal fact.
/// * `AdmittedUnconfirmed` — the item IS on the receiver queue, but no drain
///   committed its claim inside the bound. Delivery is AMBIGUOUS: it is not a
///   drop and it is emphatically not a success.
/// * `AdmissionParkTimedOut` — the receiver queue stayed at capacity for the
///   whole bound, so the item was never admitted. Nothing was delivered.
///
/// A separate type (rather than extra [`IngressDeliveryOutcome`] variants)
/// keeps the unbounded connection-ingress path's exhaustive match honest: the
/// deadline is a property of THIS call, not of the ingress queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedIngressDelivery {
    Resolved(IngressDeliveryOutcome),
    AdmittedUnconfirmed { park_bound: std::time::Duration },
    AdmissionParkTimedOut { park_bound: std::time::Duration },
}

#[derive(Debug, Clone, Copy)]
struct OutstandingClaim {
    claim_id: meerkat_core::interaction::PeerIngressClaimId,
    raw_item_id: InteractionId,
    delivery_contract: PeerIngressDeliveryContract,
    claimed_at: Instant,
}

impl ClassifiedInboxQueue {
    fn new(
        capacity: usize,
        auth_required: bool,
        trusted_peers: Arc<RwLock<TrustStore>>,
        peer_comms_handle: PeerCommsHandleSlot,
        ingress_notify: Arc<Notify>,
    ) -> Self {
        Self {
            capacity,
            closed: false,
            entries: VecDeque::new(),
            auth_required,
            trusted_peers,
            peer_comms_handle,
            phase: PeerIngressState::Absent,
            dropped_count: Arc::new(AtomicU64::new(0)),
            capacity_notify: Arc::new(Notify::new()),
            ingress_notify,
            outstanding_claim: None,
            claim_acquired_count: 0,
            handed_off_count: 0,
            durably_admitted_count: 0,
            runtime_handover_count: 0,
            terminal_outcomes: Default::default(),
            last_delivery_correlation: None,
        }
    }

    fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.dropped_count.clone()
    }

    fn capacity_notifier(&self) -> Arc<Notify> {
        self.capacity_notify.clone()
    }

    fn resolve_receive_authority(
        &self,
        facts: PeerIngressReceiveFacts,
    ) -> Option<PeerIngressReceiveAuthority> {
        let handle = self.peer_comms_handle.read().clone();
        let Some(handle) = handle else {
            tracing::warn!(
                "classified peer ingress has no machine receive authority; rejecting item"
            );
            return None;
        };

        match handle.resolve_peer_ingress_receive(facts) {
            Ok(authority) => Some(authority),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "peer ingress receive rejected by machine authority"
                );
                None
            }
        }
    }

    fn resolve_dequeue_authority(
        &self,
        facts: PeerIngressDequeueFacts,
    ) -> Option<PeerIngressDequeueAuthority> {
        let handle = self.peer_comms_handle.read().clone();
        let Some(handle) = handle else {
            tracing::warn!(
                "classified peer ingress has no machine dequeue authority; leaving phase projection unchanged"
            );
            return None;
        };

        match handle.resolve_peer_ingress_dequeue(facts) {
            Ok(authority) => Some(authority),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "peer ingress dequeue rejected by machine authority"
                );
                None
            }
        }
    }

    /// Resolve admission for a prepared item through peer-ingress authority.
    ///
    /// Plain events and external envelopes both pass queue/admission-time
    /// observations to the generated peer-ingress authority, which emits the
    /// admission outcome, optional diagnostic, and public phase.
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
    /// The queue stores only the emitted phase projection for snapshots; it
    /// does not derive phase or trust/admission legality locally.
    fn admit_peer_receive(&mut self, prepared: &PreparedIngressItem) -> AdmissionDecision {
        // Authoritative trust check at admission time. The read lock on
        // `trusted_peers` runs while we hold the queue `Mutex`, so any
        // concurrent `router.{add,remove}_trusted_peer` serializes with
        // this check — classification's stale view is ignored.
        let trusted = match &prepared.item {
            InboxItem::External { envelope } => self
                .trusted_peers
                .read()
                .contains(&envelope.from.to_peer_id()),
            InboxItem::PlainEvent { .. } => false,
        };
        let had_queued_work = !self.entries.is_empty();
        let Some(authority) = self.resolve_receive_authority(PeerIngressReceiveFacts {
            kind: prepared.kind,
            current_phase: self.phase.into(),
            auth_required: self.auth_required,
            auth_exempt: prepared.auth.is_exempt(),
            trusted,
            queued_work_present: had_queued_work,
            queue_closed: self.closed,
            queue_capacity_available: self.entries.len() < self.capacity,
        }) else {
            return AdmissionDecision {
                outcome: AdmissionOutcome::Dropped {
                    reason: DropReason::ClassificationRejected,
                },
                admission_diagnostic: None,
            };
        };

        self.phase = authority.authority_phase.into();
        let outcome = match authority.outcome {
            PeerIngressReceiveOutcome::Admitted => AdmissionOutcome::Admitted,
            PeerIngressReceiveOutcome::DroppedUntrustedSender => AdmissionOutcome::Dropped {
                reason: DropReason::UntrustedSender,
            },
            PeerIngressReceiveOutcome::DroppedSessionClosed => AdmissionOutcome::Dropped {
                reason: DropReason::SessionClosed,
            },
            PeerIngressReceiveOutcome::DroppedInboxFull => AdmissionOutcome::Dropped {
                reason: DropReason::InboxFull,
            },
        };
        AdmissionDecision {
            outcome,
            admission_diagnostic: authority.admission_diagnostic,
        }
    }

    fn admit_and_push(
        &mut self,
        prepared: PreparedIngressItem,
        ingress_completion: Option<IngressCompletion>,
    ) -> AdmissionPushDecision {
        let kind = prepared.kind;
        let decision = self.admit_peer_receive(&prepared);
        if let AdmissionOutcome::Dropped { reason } = decision.outcome {
            if let InboxItem::External { envelope } = &prepared.item {
                let request_id_for_log =
                    prepared.request_id.map(|request_id| request_id.to_string());
                tracing::warn!(
                    peer_id = %envelope.from.to_peer_id(),
                    from_peer = prepared.from_peer.as_deref().unwrap_or("<unresolved>"),
                    class = ?prepared.class,
                    kind = ?kind,
                    request_id = request_id_for_log.as_deref().unwrap_or("<none>"),
                    auth_required = self.auth_required,
                    trusted_sender = prepared.trusted_sender,
                    reason = ?reason,
                    "classified inbox dropped external peer ingress at generated admission"
                );
            }
            return AdmissionPushDecision {
                outcome: decision.outcome,
                is_actionable: false,
            };
        }

        let entry = ClassifiedInboxEntry {
            raw_item_id: prepared.raw_item_id,
            item: prepared.item,
            class: prepared.class,
            actionable: prepared.actionable,
            auth: prepared.auth,
            delivery_contract: prepared.delivery_contract,
            kind,
            from_peer: prepared.from_peer,
            ingress_fact: prepared.ingress_fact,
            lifecycle_peer: prepared.lifecycle_peer,
            request_id: prepared.request_id,
            admission_diagnostic: decision.admission_diagnostic,
            response_terminality: prepared.response_terminality,
            ingress_completion,
            text_projection: prepared.text_projection,
        };
        let is_actionable = entry.actionable;
        self.entries.push_back(entry);
        AdmissionPushDecision {
            outcome: AdmissionOutcome::Admitted,
            is_actionable,
        }
    }

    /// Project the generated peer-ingress authority phase after dequeue.
    ///
    /// The queue supplies the generated ingress kind/auth fact plus remaining
    /// occupancy. The machine decides whether the public phase changes.
    fn note_peer_dequeue(&mut self, entry: &ClassifiedInboxEntry) {
        if let Some(authority) = self.resolve_dequeue_authority(PeerIngressDequeueFacts {
            kind: entry.kind,
            auth: entry.auth,
            queued_work_remaining: !self.entries.is_empty(),
        }) {
            self.phase = authority.authority_phase.into();
        }
    }

    #[cfg(test)]
    fn drain_volatile(&mut self) -> Vec<ClassifiedInboxEntry> {
        if self.outstanding_claim.is_some() {
            return Vec::new();
        }
        let mut drained = Vec::new();
        while self.entries.front().is_some_and(|entry| {
            entry.delivery_contract == PeerIngressDeliveryContract::VolatileControl
        }) {
            if let Some(entry) = self.pop_front_volatile() {
                drained.push(entry);
            }
        }
        drained
    }

    #[cfg(test)]
    fn pop_front_volatile(&mut self) -> Option<ClassifiedInboxEntry> {
        if self.outstanding_claim.is_some()
            || self.entries.front()?.delivery_contract
                != PeerIngressDeliveryContract::VolatileControl
        {
            return None;
        }
        let entry = self.entries.pop_front();
        if let Some(entry_ref) = entry.as_ref() {
            self.handed_off_count = self.handed_off_count.saturating_add(1);
            self.note_peer_dequeue(entry_ref);
            self.capacity_notify.notify_one();
            if let Some(completion) = &entry_ref.ingress_completion {
                completion.resolve(IngressDeliveryOutcome::VolatileHandedOff);
            }
        }
        entry
    }

    fn claim_front(&mut self) -> Option<(OutstandingClaim, ClassifiedInboxEntry)> {
        if self.outstanding_claim.is_some() {
            return None;
        }
        let entry = self.entries.front()?.clone();
        let claim = OutstandingClaim {
            claim_id: meerkat_core::interaction::PeerIngressClaimId(uuid::Uuid::new_v4()),
            raw_item_id: entry.raw_item_id,
            delivery_contract: entry.delivery_contract,
            claimed_at: Instant::now(),
        };
        self.outstanding_claim = Some(claim);
        self.claim_acquired_count = self.claim_acquired_count.saturating_add(1);
        Some((claim, entry))
    }

    fn release_claim(&mut self, claim_id: meerkat_core::interaction::PeerIngressClaimId) {
        if self.outstanding_claim.map(|claim| claim.claim_id) == Some(claim_id) {
            self.outstanding_claim = None;
            self.ingress_notify.notify_waiters();
        }
    }

    fn commit_claim(
        &mut self,
        claim_id: meerkat_core::interaction::PeerIngressClaimId,
        receipt: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), meerkat_core::interaction::PeerIngressClaimCommitError> {
        let claim = self
            .outstanding_claim
            .ok_or(meerkat_core::interaction::PeerIngressClaimCommitError::NoOutstandingClaim)?;
        if claim.claim_id != claim_id {
            return Err(meerkat_core::interaction::PeerIngressClaimCommitError::ClaimMismatch);
        }
        if self.entries.front().map(|entry| entry.raw_item_id) != Some(claim.raw_item_id) {
            return Err(meerkat_core::interaction::PeerIngressClaimCommitError::HeadMismatch);
        }
        let entry_ref = self
            .entries
            .front()
            .ok_or(meerkat_core::interaction::PeerIngressClaimCommitError::HeadMismatch)?;
        if entry_ref.delivery_contract != PeerIngressDeliveryContract::DurableRuntime {
            return Err(
                meerkat_core::interaction::PeerIngressClaimCommitError::AuthorityRejected(
                    "volatile control ingress cannot be committed as durable runtime input"
                        .to_string(),
                ),
            );
        }
        let handle = self.peer_comms_handle.read().clone().ok_or_else(|| {
            meerkat_core::interaction::PeerIngressClaimCommitError::AuthorityRejected(
                "generated peer comms handle is not installed".to_string(),
            )
        })?;
        let outcome = handle
            .authorize_peer_ingress_claim_commit(
                meerkat_core::interaction::PeerIngressClaimCommitFacts {
                    claim_id,
                    raw_item_id: claim.raw_item_id,
                    class: entry_ref.class,
                },
                receipt.as_ref(),
            )
            .map_err(|error| {
                meerkat_core::interaction::PeerIngressClaimCommitError::AuthorityRejected(
                    error.to_string(),
                )
            })?;
        let entry = self
            .entries
            .pop_front()
            .ok_or(meerkat_core::interaction::PeerIngressClaimCommitError::HeadMismatch)?;
        self.handed_off_count = self.handed_off_count.saturating_add(1);
        self.outstanding_claim = None;
        self.note_peer_dequeue(&entry);
        self.capacity_notify.notify_one();
        if !self.entries.is_empty() {
            self.ingress_notify.notify_waiters();
        }

        use meerkat_core::interaction::{
            PeerIngressClaimTerminalOutcome as Outcome, PeerIngressTerminalOutcomeKind as Kind,
        };
        let interaction_id = match &entry.item {
            InboxItem::External { envelope } => Some(InteractionId(envelope.id)),
            InboxItem::PlainEvent { interaction_id, .. } => {
                Some(InteractionId(interaction_id.unwrap_or(entry.raw_item_id.0)))
            }
        };
        let (kind, runtime_input_id, existing_runtime_input_id) = match outcome {
            Outcome::RuntimeAccepted { input_id } => {
                self.durably_admitted_count = self.durably_admitted_count.saturating_add(1);
                self.runtime_handover_count = self.runtime_handover_count.saturating_add(1);
                self.terminal_outcomes.accepted = self.terminal_outcomes.accepted.saturating_add(1);
                (Kind::Accepted, Some(input_id), None)
            }
            Outcome::RuntimeDeduplicated {
                input_id,
                existing_id,
            } => {
                self.durably_admitted_count = self.durably_admitted_count.saturating_add(1);
                self.runtime_handover_count = self.runtime_handover_count.saturating_add(1);
                self.terminal_outcomes.deduplicated =
                    self.terminal_outcomes.deduplicated.saturating_add(1);
                (Kind::Deduplicated, Some(input_id), Some(existing_id))
            }
            Outcome::RuntimeRejected { input_id } => {
                self.runtime_handover_count = self.runtime_handover_count.saturating_add(1);
                self.terminal_outcomes.rejected = self.terminal_outcomes.rejected.saturating_add(1);
                (Kind::Rejected, Some(input_id), None)
            }
        };
        self.last_delivery_correlation =
            Some(meerkat_core::interaction::PeerIngressDeliveryCorrelation {
                raw_item_id: entry.raw_item_id,
                interaction_id,
                runtime_input_id,
                existing_runtime_input_id,
                outcome: kind,
            });
        if let Some(completion) = entry.ingress_completion {
            completion.resolve(IngressDeliveryOutcome::DurablyResolved(kind));
        }
        Ok(())
    }

    fn handoff_volatile_claim(
        &mut self,
        claim_id: meerkat_core::interaction::PeerIngressClaimId,
    ) -> Result<(), meerkat_core::interaction::PeerIngressClaimCommitError> {
        let claim = self
            .outstanding_claim
            .ok_or(meerkat_core::interaction::PeerIngressClaimCommitError::NoOutstandingClaim)?;
        if claim.claim_id != claim_id {
            return Err(meerkat_core::interaction::PeerIngressClaimCommitError::ClaimMismatch);
        }
        if self.entries.front().map(|entry| entry.raw_item_id) != Some(claim.raw_item_id) {
            return Err(meerkat_core::interaction::PeerIngressClaimCommitError::HeadMismatch);
        }
        if self.entries.front().is_some_and(|entry| {
            entry.delivery_contract != PeerIngressDeliveryContract::VolatileControl
        }) {
            return Err(
                meerkat_core::interaction::PeerIngressClaimCommitError::AuthorityRejected(
                    "durable runtime ingress requires an AcceptWithCompletion receipt".to_string(),
                ),
            );
        }
        let entry = self
            .entries
            .pop_front()
            .ok_or(meerkat_core::interaction::PeerIngressClaimCommitError::HeadMismatch)?;
        self.handed_off_count = self.handed_off_count.saturating_add(1);
        self.outstanding_claim = None;
        self.note_peer_dequeue(&entry);
        self.capacity_notify.notify_one();
        if !self.entries.is_empty() {
            self.ingress_notify.notify_waiters();
        }
        if let Some(completion) = entry.ingress_completion {
            completion.resolve(IngressDeliveryOutcome::VolatileHandedOff);
        }
        Ok(())
    }

    fn close(&mut self) {
        self.closed = true;
        self.outstanding_claim = None;
        for entry in &mut self.entries {
            entry.ingress_completion = None;
        }
        self.capacity_notify.notify_waiters();
        self.ingress_notify.notify_waiters();
    }

    fn snapshot(&self) -> PeerIngressQueueSnapshot {
        let queued_entries: Vec<_> = self.entries.iter().map(snapshot_entry).collect();
        let actionable_count = queued_entries
            .iter()
            .filter(|entry| entry.actionable)
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
            outstanding_claim: self.outstanding_claim.map(|claim| {
                meerkat_core::interaction::PeerIngressClaimSnapshot {
                    claim_id: claim.claim_id,
                    raw_item_id: claim.raw_item_id,
                    delivery_contract: claim.delivery_contract,
                    age: claim.claimed_at.elapsed(),
                }
            }),
            claim_acquired_count: self.claim_acquired_count,
            handed_off_count: self.handed_off_count,
            durably_admitted_count: self.durably_admitted_count,
            runtime_handover_count: self.runtime_handover_count,
            terminal_outcomes: self.terminal_outcomes,
            last_delivery_correlation: self.last_delivery_correlation.clone(),
            queued_entries,
        }
    }

    fn runtime_snapshot(&self) -> (PeerIngressQueueSnapshot, PeerIngressState) {
        (self.snapshot(), self.phase)
    }
}

pub(crate) struct ClassifiedInboxClaim {
    pub(crate) claim_id: meerkat_core::interaction::PeerIngressClaimId,
    pub(crate) entry: ClassifiedInboxEntry,
    pub(crate) lease: Arc<ClassifiedInboxClaimLease>,
}

pub(crate) struct ClassifiedInboxClaimLease {
    queue: Arc<Mutex<ClassifiedInboxQueue>>,
}

impl meerkat_core::interaction::PeerIngressClaimLease for ClassifiedInboxClaimLease {
    fn commit(
        &self,
        claim_id: meerkat_core::interaction::PeerIngressClaimId,
        receipt: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), meerkat_core::interaction::PeerIngressClaimCommitError> {
        self.queue.lock().commit_claim(claim_id, receipt)
    }

    fn handoff_volatile(
        &self,
        claim_id: meerkat_core::interaction::PeerIngressClaimId,
    ) -> Result<(), meerkat_core::interaction::PeerIngressClaimCommitError> {
        self.queue.lock().handoff_volatile_claim(claim_id)
    }

    fn release(&self, claim_id: meerkat_core::interaction::PeerIngressClaimId) {
        self.queue.lock().release_claim(claim_id);
    }
}

/// The receiving end of the inbox, held by the agent loop.
pub struct Inbox {
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
    /// Create a raw transport-only inbox, returning both the inbox and a sender.
    ///
    /// Raw inboxes do not own Meerkat peer/event admission authority and have
    /// no delivery queue: sending semantic `InboxItem`s through the public
    /// sender fails closed. Runtime peer/event ingress must use
    /// [`Self::new_classified`] with an installed machine handle.
    pub fn new_transport_only() -> (Self, InboxSender) {
        let notify = Arc::new(Notify::new());
        (
            Inbox {
                notify: notify.clone(),
                classified_queue: None,
                actionable_notify: None,
                dropped_count: None,
            },
            InboxSender {
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
    /// The classified queue is the sole delivery path; `send_classified()`
    /// enqueues only on that queue.
    pub(crate) fn new_classified(
        context: Arc<IngressClassificationContext>,
    ) -> (Self, InboxSender) {
        Self::new_classified_with_queue_capacity(context, DEFAULT_INBOX_CAPACITY)
    }

    #[cfg(test)]
    pub(crate) fn new_classified_with_capacity_for_test(
        context: Arc<IngressClassificationContext>,
        capacity: usize,
    ) -> (Self, InboxSender) {
        Self::new_classified_with_queue_capacity(context, capacity)
    }

    fn new_classified_with_queue_capacity(
        context: Arc<IngressClassificationContext>,
        capacity: usize,
    ) -> (Self, InboxSender) {
        let notify = Arc::new(Notify::new());
        let actionable_notify = Arc::new(Notify::new());
        let queue = ClassifiedInboxQueue::new(
            capacity,
            context.require_peer_auth,
            context.trusted_peers.clone(),
            context.peer_comms_handle.clone(),
            notify.clone(),
        );
        let dropped_count = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        (
            Inbox {
                notify: notify.clone(),
                classified_queue: Some(classified_queue.clone()),
                actionable_notify: Some(actionable_notify.clone()),
                dropped_count: Some(dropped_count.clone()),
            },
            InboxSender {
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

    /// Try to drain the contiguous volatile-control prefix without blocking.
    /// Durable runtime entries are never removed through this legacy control
    /// seam and an outstanding exact claim blocks destructive handoff.
    #[cfg(test)]
    pub(crate) fn try_drain_classified(&mut self) -> Vec<ClassifiedInboxEntry> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().drain_volatile())
            .unwrap_or_default()
    }

    /// Try to receive one volatile-control entry without blocking.
    #[cfg(test)]
    pub(crate) fn try_recv_one_classified(&mut self) -> Option<ClassifiedInboxEntry> {
        self.classified_queue.as_ref()?.lock().pop_front_volatile()
    }

    /// Non-destructively claim the current FIFO head.
    pub(crate) fn try_claim_one_classified(&self) -> Option<ClassifiedInboxClaim> {
        let queue = self.classified_queue.as_ref()?.clone();
        let (claim, entry) = queue.lock().claim_front()?;
        Some(ClassifiedInboxClaim {
            claim_id: claim.claim_id,
            entry,
            lease: Arc::new(ClassifiedInboxClaimLease { queue }),
        })
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

    /// Test-only payload snapshot. Production observers use the typed queue
    /// snapshot; tests that need exact bodies may clone entries without
    /// bypassing durable claim authority or changing FIFO state.
    #[cfg(test)]
    pub(crate) fn test_classified_entries_snapshot(&self) -> Vec<ClassifiedInboxEntry> {
        self.classified_queue
            .as_ref()
            .map(|queue| queue.lock().entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn peer_runtime_snapshot(
        &self,
    ) -> Option<(PeerIngressQueueSnapshot, PeerIngressState)> {
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

    /// Test-only: ask the queue's shared trust store whether `pubkey` is
    /// currently trusted. The queue does not own a local copy — this
    /// reads from the same `Arc<RwLock<TrustStore>>` that the router
    /// mutates and that the classifier consults.
    #[cfg(test)]
    pub(crate) fn peer_authority_trusts_peer_for_test(
        &self,
        pubkey: &crate::identity::PubKey,
    ) -> Option<bool> {
        self.classified_queue.as_ref().map(|queue| {
            queue
                .lock()
                .trusted_peers
                .read()
                .contains(&pubkey.to_peer_id())
        })
    }
}

impl InboxSender {
    /// Whether two sender handles own the exact same inbox generation.
    ///
    /// Participant names and signing keys can be deliberately reused across
    /// a runtime replacement. The notify Arc is minted once per inbox, so
    /// pointer identity is the narrow lease witness needed for compare-remove
    /// cleanup without exposing queue internals.
    pub(crate) fn same_inbox(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.notify, &other.notify)
    }

    /// **Test-only seam for C-H3.** Simulate the classify-then-admit
    /// ordering with an explicit pause between the two steps so a test
    /// can mutate the trust set in the gap.
    ///
    /// Returns `None` on non-classified inboxes (there is no admission
    /// step to exercise). Marked `#[doc(hidden)]` so it is callable from
    /// integration tests without appearing on the public API surface.
    #[cfg(test)]
    pub(crate) fn classified_admit_with_pause_for_test(
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
        let decision = {
            let mut queue = classified_queue.lock();
            queue.admit_and_push(result, None)
        };
        match decision.outcome {
            AdmissionOutcome::Admitted => {
                self.notify.notify_waiters();
                AdmissionOutcome::Admitted
            }
            AdmissionOutcome::Dropped { reason } => self.record_drop(reason),
        }
    }

    /// Admit one external envelope at the inbox seam.
    ///
    /// This keeps trust ownership inside `InboxSender` instead of the
    /// transport task. Classified runtimes route through `send_classified()`;
    /// raw inboxes are transport-only and cannot assert semantic peer ingress
    /// admission without machine authority.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_connection_ingress(
        &self,
        envelope: Envelope,
        require_peer_auth: bool,
    ) -> AdmissionOutcome {
        if self.classification_context.is_some() {
            return self.send_classified(InboxItem::External { envelope });
        }

        let _ = require_peer_auth;
        tracing::warn!(
            peer_id = %envelope.from.to_peer_id(),
            reason = ?DropReason::ClassificationRejected,
            "raw inbox rejected external peer ingress because no machine classification context is installed"
        );
        self.record_drop(DropReason::ClassificationRejected)
    }

    /// Admit a connection envelope, then wait until its exact queue claim is
    /// either durably resolved by runtime admission or explicitly handed to a
    /// volatile control consumer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn send_connection_ingress_for_delivery(
        &self,
        envelope: Envelope,
        require_peer_auth: bool,
    ) -> IngressDeliveryOutcome {
        let _ = require_peer_auth;
        self.send_classified_for_delivery(InboxItem::External { envelope }, None)
            .await
    }

    /// Admit one envelope received on a TCP connection together with the
    /// kernel-observed remote socket address.
    ///
    /// Transport evidence stays out-of-band from [`InboxItem`] so it cannot be
    /// serialized, replayed, or sender-forged. Open-auth listeners deliberately
    /// discard the source metadata for callback-routing purposes.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn send_tcp_connection_ingress_for_delivery(
        &self,
        envelope: Envelope,
        require_peer_auth: bool,
        observed_source: SocketAddr,
    ) -> IngressDeliveryOutcome {
        let observed_source = require_peer_auth.then_some(observed_source);
        self.send_classified_for_delivery(InboxItem::External { envelope }, observed_source)
            .await
    }

    /// Send an item to the inbox.
    ///
    /// On classified runtimes, delegates to `send_classified()` so the item
    /// goes through the classified queue (the sole consumer). On raw inboxes,
    /// semantic peer/event items fail closed because no machine authority is
    /// installed to classify admission or public result facts.
    ///
    /// Returns a typed `AdmissionOutcome`. Drops are never surfaced as `Ok(())`.
    #[cfg(test)]
    pub(crate) fn send(&self, item: InboxItem) -> AdmissionOutcome {
        // If classification context is available, route through classified path.
        if self.classification_context.is_some() {
            return self.send_classified(item);
        }
        let _ = item;
        tracing::warn!(
            reason = ?DropReason::ClassificationRejected,
            "raw inbox rejected semantic item because no machine classification context is installed"
        );
        self.record_drop(DropReason::ClassificationRejected)
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
    pub(crate) fn send_classified(&self, item: InboxItem) -> AdmissionOutcome {
        self.send_classified_with_observed_tcp_source(item, None)
    }

    fn send_classified_with_observed_tcp_source(
        &self,
        item: InboxItem,
        observed_tcp_source: Option<SocketAddr>,
    ) -> AdmissionOutcome {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            tracing::warn!(
                reason = ?DropReason::ClassificationRejected,
                "classified inbox send requested without machine classification context"
            );
            return self.record_drop(DropReason::ClassificationRejected);
        };

        let result = match ctx.prepare_with_observed_tcp_source(item, observed_tcp_source) {
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
        // C-H3 — generated admission (trust re-check, auth-exempt bypass,
        // capacity/closed result, phase update) and enqueue happen inside a
        // single queue-lock scope so no handwritten post-authority path can
        // change the public admission result.
        let decision = {
            let mut queue = classified_queue.lock();
            queue.admit_and_push(result, None)
        };
        match decision.outcome {
            AdmissionOutcome::Admitted => {
                // Fire actionable notify only for actionable classes.
                if decision.is_actionable
                    && let Some(ref actionable) = self.actionable_notify
                {
                    actionable.notify_waiters();
                }
                // Always fire broad notify.
                self.notify.notify_waiters();
                AdmissionOutcome::Admitted
            }
            AdmissionOutcome::Dropped { reason } => self.record_drop(reason),
        }
    }

    /// Send an item through the same admission path as [`Self::send`], but
    /// await queue capacity instead of reporting `InboxFull`.
    ///
    /// This is used by runtime-originated peer sends where backpressure is
    /// preferable to semantic loss. Policy and closed-queue failures remain
    /// typed `Dropped` outcomes; only capacity waits.
    pub(crate) async fn send_wait(&self, item: InboxItem) -> AdmissionOutcome {
        if self.classification_context.is_some() {
            return self.send_classified_wait(item).await;
        }
        let _ = item;
        tracing::warn!(
            reason = ?DropReason::ClassificationRejected,
            "raw inbox rejected semantic item while waiting because no machine classification context is installed"
        );
        self.record_drop(DropReason::ClassificationRejected)
    }

    /// Wait for queue capacity and then for authoritative durable admission,
    /// under a single caller-supplied deadline.
    ///
    /// Used by in-process delivery, whose successful return has the same
    /// meaning as a signed transport ack. BOTH parks in this call are resolved
    /// only by the receiver's drain (capacity opens on dequeue, the receipt
    /// resolves on claim commit), so an undrained receiver would otherwise make
    /// sender liveness a function of receiver-drain liveness with no bound.
    /// `park_bound` is that bound; it covers the whole call, not each park
    /// separately, so the caller's deadline means one thing.
    ///
    /// Expiry is classified by WHERE it fired: before admission nothing was
    /// delivered; after admission delivery is ambiguous. See
    /// [`BoundedIngressDelivery`].
    pub(crate) async fn send_wait_for_delivery(
        &self,
        item: InboxItem,
        park_bound: std::time::Duration,
    ) -> BoundedIngressDelivery {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            return BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                reason: DropReason::ClassificationRejected,
            });
        };
        let Some(prepared) = ctx.prepare(item) else {
            return BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                reason: DropReason::ClassificationRejected,
            });
        };
        // ONE budget across both parks, tracked as remaining time rather than
        // as a `tokio::time::Instant` deadline: the wasm glue behind
        // `crate::tokio` exposes neither `Instant` nor `timeout_at`, and this
        // path is compiled for wasm32. `time_compat::Instant` is the
        // browser-safe monotonic clock.
        let park_started = meerkat_core::time_compat::Instant::now();
        let remaining_budget = || park_bound.saturating_sub(park_started.elapsed());
        let kind = prepared.kind;
        let mut prepared = Some(prepared);
        let capacity_notify = classified_queue.lock().capacity_notifier();
        let (completion, receipt) = IngressCompletion::new();
        let mut completion = Some(completion);

        loop {
            let notified = capacity_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let admitted = {
                let mut queue = classified_queue.lock();
                if queue.closed {
                    return BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                        reason: DropReason::SessionClosed,
                    });
                }
                if queue.entries.len() < queue.capacity {
                    let Some(prepared) = prepared.take() else {
                        tracing::error!(kind = ?kind, "durable send lost prepared ingress item");
                        return BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                            reason: DropReason::SessionClosed,
                        });
                    };
                    Some(queue.admit_and_push(prepared, completion.take()))
                } else {
                    None
                }
            };
            if let Some(decision) = admitted {
                match decision.outcome {
                    AdmissionOutcome::Dropped { reason } => {
                        return BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                            reason,
                        });
                    }
                    AdmissionOutcome::Admitted => {
                        if decision.is_actionable
                            && let Some(actionable) = &self.actionable_notify
                        {
                            actionable.notify_waiters();
                        }
                        self.notify.notify_waiters();
                        return match tokio::time::timeout(remaining_budget(), receipt.wait()).await
                        {
                            Ok(outcome) => BoundedIngressDelivery::Resolved(outcome),
                            Err(_) => BoundedIngressDelivery::AdmittedUnconfirmed { park_bound },
                        };
                    }
                }
            }
            run_classified_send_wait_before_await_hook();
            // The capacity park is bounded by the SAME deadline as the receipt
            // park: a receiver whose drain is dead never dequeues, so
            // `capacity_notify` never fires and this await would otherwise be
            // the second unbounded park in this call.
            if tokio::time::timeout(remaining_budget(), notified.as_mut())
                .await
                .is_err()
            {
                return BoundedIngressDelivery::AdmissionParkTimedOut { park_bound };
            }
        }
    }

    async fn send_classified_wait(&self, item: InboxItem) -> AdmissionOutcome {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            tracing::warn!(
                reason = ?DropReason::ClassificationRejected,
                "classified inbox send_wait requested without machine classification context"
            );
            return self.record_drop(DropReason::ClassificationRejected);
        };

        let result = match ctx.prepare(item) {
            Some(r) => r,
            None => {
                tracing::warn!(
                    reason = ?DropReason::ClassificationRejected,
                    "classified inbox dropped item at classification stage"
                );
                return self.record_drop(DropReason::ClassificationRejected);
            }
        };
        let kind = result.kind;
        let mut prepared = Some(result);
        let capacity_notify = classified_queue.lock().capacity_notifier();

        loop {
            let notified = capacity_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut queue = classified_queue.lock();
                if queue.closed {
                    let Some(result) = prepared.take() else {
                        tracing::error!(
                            kind = ?kind,
                            reason = ?DropReason::SessionClosed,
                            "classified inbox send_wait reached closed admission after item was already consumed"
                        );
                        return self.record_drop(DropReason::SessionClosed);
                    };
                    let decision = queue.admit_and_push(result, None);
                    if let AdmissionOutcome::Dropped { reason } = decision.outcome {
                        return self.record_drop(reason);
                    }
                    if decision.is_actionable
                        && let Some(ref actionable) = self.actionable_notify
                    {
                        actionable.notify_waiters();
                    }
                    self.notify.notify_waiters();
                    return AdmissionOutcome::Admitted;
                }
                if queue.entries.len() < queue.capacity {
                    let Some(result) = prepared.take() else {
                        tracing::error!(
                            kind = ?kind,
                            reason = ?DropReason::SessionClosed,
                            "classified inbox send_wait reached admission after item was already consumed"
                        );
                        return self.record_drop(DropReason::SessionClosed);
                    };
                    let decision = queue.admit_and_push(result, None);
                    if let AdmissionOutcome::Dropped { reason } = decision.outcome {
                        return self.record_drop(reason);
                    }
                    if decision.is_actionable
                        && let Some(ref actionable) = self.actionable_notify
                    {
                        actionable.notify_waiters();
                    }
                    self.notify.notify_waiters();
                    return AdmissionOutcome::Admitted;
                }
            }
            run_classified_send_wait_before_await_hook();
            notified.await;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn send_classified_for_delivery(
        &self,
        item: InboxItem,
        observed_tcp_source: Option<SocketAddr>,
    ) -> IngressDeliveryOutcome {
        let (Some(ctx), Some(classified_queue)) =
            (&self.classification_context, &self.classified_queue)
        else {
            return IngressDeliveryOutcome::Dropped {
                reason: DropReason::ClassificationRejected,
            };
        };
        let Some(prepared) = ctx.prepare_with_observed_tcp_source(item, observed_tcp_source) else {
            return IngressDeliveryOutcome::Dropped {
                reason: DropReason::ClassificationRejected,
            };
        };
        let (completion, receipt) = IngressCompletion::new();
        let decision = classified_queue
            .lock()
            .admit_and_push(prepared, Some(completion));
        match decision.outcome {
            AdmissionOutcome::Dropped { reason } => IngressDeliveryOutcome::Dropped { reason },
            AdmissionOutcome::Admitted => {
                if decision.is_actionable
                    && let Some(actionable) = &self.actionable_notify
                {
                    actionable.notify_waiters();
                }
                self.notify.notify_waiters();
                receipt.wait().await
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
        actionable: entry.actionable,
        delivery_contract: entry.delivery_contract,
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
                objective_id: None,
                content_taint: None,
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
                objective_id: None,
                content_taint: None,
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 42}),
                blocks: None,
                reply_endpoint: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        }
    }

    fn make_response_envelope(in_reply_to: Uuid) -> Envelope {
        let mut envelope = make_test_envelope();
        envelope.kind = MessageKind::Response {
            objective_id: None,
            content_taint: None,
            in_reply_to,
            status: crate::types::Status::Completed,
            result: serde_json::json!({"ok": true}),
            blocks: None,
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
        let (inbox, _sender) = Inbox::new_transport_only();
        // Inbox exists and has the receiver
        drop(inbox);
    }

    #[test]
    fn test_inbox_sender_struct() {
        let (_inbox, sender) = Inbox::new_transport_only();
        // Sender exists and can be cloned
        let _sender2 = sender;
    }

    #[test]
    fn test_inbox_new() {
        let (inbox, sender) = Inbox::new_transport_only();
        // Both parts exist
        drop(inbox);
        drop(sender);
    }

    #[tokio::test]
    async fn test_raw_inbox_sender_send_fails_closed_without_machine_context() {
        let (_inbox, sender) = Inbox::new_transport_only();
        let item = InboxItem::External {
            envelope: make_test_envelope(),
        };
        let result = sender.send(item);
        assert_eq!(
            result,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            }
        );
    }

    #[tokio::test]
    async fn test_raw_inbox_repeated_sends_fail_closed() {
        let (_inbox, sender) = Inbox::new_transport_only();

        // Raw inboxes cannot classify semantic peer ingress without machine authority.
        for i in 0..3 {
            let mut envelope = make_test_envelope();
            envelope.id = Uuid::from_u128(i as u128);
            assert_eq!(
                sender.send(InboxItem::External { envelope }),
                AdmissionOutcome::Dropped {
                    reason: DropReason::ClassificationRejected
                }
            );
        }
    }

    #[test]
    fn test_sender_error_on_closed_inbox() {
        let (inbox, sender) = Inbox::new_transport_only();
        drop(inbox); // Close the inbox

        let result = sender.send(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert!(matches!(
            result,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
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
        let (inbox, _sender) = Inbox::new_transport_only();
        // Inbox should expose a Notify
        let notify = inbox.notify();
        // Arc should be clonable
        drop(notify);
    }

    #[tokio::test]
    async fn test_raw_sender_does_not_notify_after_rejected_send() {
        let (inbox, sender) = Inbox::new_transport_only();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let notified = notify.notified();

        let outcome = sender.send(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            }
        );

        let result = tokio::time::timeout(std::time::Duration::from_millis(50), notified).await;

        assert!(
            result.is_err(),
            "raw rejected send must not notify inbox consumers"
        );
    }

    #[tokio::test]
    async fn test_raw_rejected_send_does_not_wake_waiting_task() {
        let (inbox, sender) = Inbox::new_transport_only();
        let notify = inbox.notify();

        // Spawn a task that waits for notification
        let mut handle = tokio::spawn(async move {
            notify.notified().await;
            "woken"
        });

        // Give the task time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let outcome = sender.send(InboxItem::External {
            envelope: make_test_envelope(),
        });
        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            }
        );

        let result = tokio::time::timeout(std::time::Duration::from_millis(50), &mut handle).await;

        assert!(result.is_err(), "Task should remain parked");
        handle.abort();
    }

    // === Classified inbox tests ===

    use crate::classify::{IngressClassificationContext, test_support};
    use crate::trust::{TrustEntry, TrustStore};
    use std::sync::atomic::AtomicUsize;

    fn make_classification_context(
        trusted: TrustStore,
        require_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        make_classification_context_with_machine(
            trusted,
            require_auth,
            Some(test_support::runtime_peer_comms_handle()),
        )
    }

    fn make_classification_context_without_machine(
        trusted: TrustStore,
        require_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        make_classification_context_with_machine(trusted, require_auth, None)
    }

    fn make_classification_context_with_machine(
        trusted: TrustStore,
        require_auth: bool,
        peer_comms_handle: Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>>,
    ) -> Arc<IngressClassificationContext> {
        Arc::new(IngressClassificationContext {
            require_peer_auth: require_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted)),
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(peer_comms_handle)),
            inproc_namespace: None,
            durable_runtime_consumer: true,
        })
    }

    fn make_classified_queue(
        capacity: usize,
        ctx: &Arc<IngressClassificationContext>,
    ) -> ClassifiedInboxQueue {
        ClassifiedInboxQueue::new(
            capacity,
            ctx.require_peer_auth,
            ctx.trusted_peers.clone(),
            ctx.peer_comms_handle.clone(),
            Arc::new(Notify::new()),
        )
    }

    fn make_trusted(name: &str, pubkey: &PubKey) -> TrustStore {
        let mut store = TrustStore::new();
        store
            .insert(TrustEntry {
                peer_id: pubkey.to_peer_id(),
                name: meerkat_core::comms::PeerName::new(name).expect("valid peer name"),
                pubkey: *pubkey,
                address: meerkat_core::comms::PeerAddress::parse("inproc://test")
                    .expect("valid peer address"),
                meta: crate::PeerMeta::default(),
            })
            .expect("trusted test peer should insert");
        store
    }

    struct RejectingPeerCommsHandle {
        external_calls: AtomicUsize,
        delegate: Arc<dyn meerkat_core::handles::PeerCommsHandle>,
    }

    impl RejectingPeerCommsHandle {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                delegate: test_support::runtime_peer_comms_handle(),
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
            self.delegate.classify_plain_event(facts)
        }

        fn resolve_peer_ingress_receive(
            &self,
            facts: meerkat_core::PeerIngressReceiveFacts,
        ) -> Result<
            meerkat_core::PeerIngressReceiveAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.delegate.resolve_peer_ingress_receive(facts)
        }

        fn resolve_peer_ingress_dequeue(
            &self,
            facts: meerkat_core::PeerIngressDequeueFacts,
        ) -> Result<
            meerkat_core::PeerIngressDequeueAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.delegate.resolve_peer_ingress_dequeue(facts)
        }

        fn set_peer_ingress_context(
            &self,
            keep_alive: bool,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.delegate.set_peer_ingress_context(keep_alive)
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
            objective_id: None,
            content_taint: None,
            intent: "review".to_string(),
            params: serde_json::json!({}),
            blocks: None,
            reply_endpoint: None,
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
            objective_id: None,
            content_taint: None,
            in_reply_to: Uuid::new_v4(),
            status: crate::types::Status::Completed,
            result: serde_json::json!({}),
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
        let ctx = make_classification_context(TrustStore::new(), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let actionable = inbox.classified_actionable_notify().unwrap();

        sender
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
                input_identity: None,
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
            objective_id: None,
            content_taint: None,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({"peer": "new-agent"}),
            blocks: None,
            reply_endpoint: None,
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
    async fn test_classified_durable_head_requires_a_claim() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (mut inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        let entries = inbox.test_classified_entries_snapshot();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].raw_item_id.to_string().is_empty());
        assert_eq!(
            entries[0].class,
            meerkat_core::PeerInputClass::ActionableMessage
        );
        assert!(
            inbox.try_drain_classified().is_empty(),
            "legacy volatile drain must retain DurableRuntime ingress"
        );
        assert_eq!(inbox.test_classified_entries_snapshot().len(), 1);
    }

    #[tokio::test]
    async fn durable_claim_cancel_retains_the_exact_head() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let envelope = make_test_envelope();
        let raw_item_id = InteractionId(envelope.id);
        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let claimed = inbox
            .try_claim_one_classified()
            .expect("durable head claim");
        let claim = meerkat_core::PeerIngressQueueClaim::from_comms_queue(
            claimed.claim_id,
            claimed.entry.raw_item_id,
            claimed.entry.class,
            claimed.entry.delivery_contract,
            None,
            claimed.lease,
        );
        drop(claim);

        let entries = inbox.test_classified_entries_snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].raw_item_id, raw_item_id);
    }

    #[tokio::test]
    async fn wrong_claim_id_is_rejected_without_releasing_or_removing_head() {
        use meerkat_core::interaction::PeerIngressClaimLease as _;

        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();

        let claimed = inbox
            .try_claim_one_classified()
            .expect("durable head claim");
        let wrong = meerkat_core::interaction::PeerIngressClaimId(Uuid::new_v4());
        assert_eq!(
            claimed.lease.handoff_volatile(wrong),
            Err(meerkat_core::interaction::PeerIngressClaimCommitError::ClaimMismatch)
        );
        assert!(
            inbox.try_claim_one_classified().is_none(),
            "wrong claim id must leave the original lease outstanding"
        );
        claimed.lease.release(claimed.claim_id);
        assert_eq!(inbox.test_classified_entries_snapshot().len(), 1);
    }

    #[tokio::test]
    async fn released_fifo_claim_reacquires_the_same_head_with_a_new_claim_id() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let first = make_test_envelope();
        let first_raw_id = InteractionId(first.id);
        let second = make_test_envelope();
        let second_raw_id = InteractionId(second.id);
        for envelope in [first, second] {
            sender
                .send_classified(InboxItem::External { envelope })
                .into_result()
                .unwrap();
        }

        let first_claim = inbox.try_claim_one_classified().expect("first FIFO claim");
        let first_claim_id = first_claim.claim_id;
        assert_eq!(first_claim.entry.raw_item_id, first_raw_id);
        let claim = meerkat_core::PeerIngressQueueClaim::from_comms_queue(
            first_claim.claim_id,
            first_claim.entry.raw_item_id,
            first_claim.entry.class,
            first_claim.entry.delivery_contract,
            None,
            first_claim.lease,
        );
        drop(claim);

        let reacquired = inbox
            .try_claim_one_classified()
            .expect("released FIFO head must be claimable again");
        assert_ne!(reacquired.claim_id, first_claim_id);
        assert_eq!(reacquired.entry.raw_item_id, first_raw_id);
        assert_ne!(reacquired.entry.raw_item_id, second_raw_id);
    }

    #[tokio::test]
    async fn durable_raw_item_id_is_stable_across_snapshot_and_claim_cycles() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);
        let envelope = make_test_envelope();
        let expected = InteractionId(envelope.id);
        sender
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let snapshot_raw = inbox.test_classified_entries_snapshot()[0].raw_item_id;
        let first = inbox.try_claim_one_classified().expect("first claim");
        assert_eq!(first.entry.raw_item_id, expected);
        let first_core = meerkat_core::PeerIngressQueueClaim::from_comms_queue(
            first.claim_id,
            first.entry.raw_item_id,
            first.entry.class,
            first.entry.delivery_contract,
            None,
            first.lease,
        );
        drop(first_core);
        let second = inbox.try_claim_one_classified().expect("second claim");

        assert_eq!(snapshot_raw, expected);
        assert_eq!(second.entry.raw_item_id, expected);
    }

    #[tokio::test]
    async fn test_classified_snapshot_is_non_destructive() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let (inbox, sender) = Inbox::new_classified(ctx);

        sender
            .send_classified(InboxItem::External {
                envelope: make_test_envelope(),
            })
            .into_result()
            .unwrap();
        sender
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
                input_identity: None,
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

        assert_eq!(
            inbox.test_classified_entries_snapshot().len(),
            2,
            "snapshot must not drain queued ingress"
        );
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
                objective_id: None,
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
                input_identity: None,
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
        let ctx = make_classification_context(TrustStore::new(), false);
        let (inbox, sender) = Inbox::new_classified(ctx);

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

        let drained = inbox.test_classified_entries_snapshot();
        assert_eq!(
            drained.len(),
            1,
            "admission was governed by authority, not reconstructed from the snapshot"
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
        let ctx = make_classification_context(TrustStore::new(), true);
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
    async fn classification_then_trust_revoke_then_admission_rechecks_live_trust() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), true);
        let (inbox, sender) = Inbox::new_classified(ctx.clone());
        let mut envelope = make_test_envelope();
        envelope.from = sender_pubkey;

        let outcome =
            sender.classified_admit_with_pause_for_test(InboxItem::External { envelope }, || {
                *ctx.trusted_peers.write() = TrustStore::new();
            });

        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::UntrustedSender
            },
            "admission must re-read trust after classification and reject the T1 revoke"
        );
        assert_eq!(inbox.test_classified_entries_snapshot().len(), 0);
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
        let (inbox, sender) = Inbox::new_classified(ctx);

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
        assert_eq!(inbox.test_classified_entries_snapshot().len(), 0);
        assert_eq!(machine.external_calls(), 1);
    }

    #[tokio::test]
    async fn test_runtime_required_response_without_machine_authority_fails_closed() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx =
            make_classification_context_without_machine(make_trusted("peer", &sender_pubkey), true);
        let (inbox, sender) = Inbox::new_classified(ctx);

        let outcome = sender.send_classified(InboxItem::External {
            envelope: make_response_envelope(Uuid::new_v4()),
        });

        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            },
            "peer response must not be accepted or terminalized without machine authority"
        );
        assert_eq!(inbox.dropped_count(), Some(1));
        assert_eq!(inbox.test_classified_entries_snapshot().len(), 0);
    }

    #[tokio::test]
    async fn test_classified_full_queue_returns_typed_drop_reason() {
        // InboxFull drops surface as typed `Dropped { InboxFull }` with the
        // counter bumped. Build a classified queue at capacity 1 directly:
        // admit the first envelope, reject the second.
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        // `new_classified` uses DEFAULT_INBOX_CAPACITY; to exercise full we
        // need a direct construction path. Fall back to building the queue
        // directly and then driving it.
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(1, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
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

    #[tokio::test]
    async fn test_classified_send_wait_backpressures_until_capacity_opens() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(1, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
            notify,
            classification_context: Some(ctx),
            classified_queue: Some(classified_queue.clone()),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        assert_eq!(
            sender.send_classified(InboxItem::External {
                envelope: make_response_envelope(Uuid::new_v4()),
            }),
            AdmissionOutcome::Admitted
        );

        let waiting_sender = sender.clone();
        let waiting = tokio::spawn(async move {
            waiting_sender
                .send_wait(InboxItem::External {
                    envelope: make_response_envelope(Uuid::new_v4()),
                })
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !waiting.is_finished(),
            "send_wait must wait instead of dropping when the classified queue is full"
        );

        assert!(classified_queue.lock().pop_front_volatile().is_some());
        assert_eq!(
            waiting.await.expect("send_wait task should complete"),
            AdmissionOutcome::Admitted
        );
        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "backpressured send should not increment the drop counter"
        );
    }

    #[tokio::test]
    async fn test_classified_send_wait_rechecks_trust_after_capacity_opens() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), true);
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(1, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
            notify,
            classification_context: Some(ctx.clone()),
            classified_queue: Some(classified_queue.clone()),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        assert_eq!(
            sender.send_classified(InboxItem::External {
                envelope: make_response_envelope(Uuid::new_v4()),
            }),
            AdmissionOutcome::Admitted
        );

        let waiting_sender = sender.clone();
        let waiting = tokio::spawn(async move {
            waiting_sender
                .send_wait(InboxItem::External {
                    envelope: make_response_envelope(Uuid::new_v4()),
                })
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !waiting.is_finished(),
            "send_wait should be parked on capacity before the revoke"
        );

        *ctx.trusted_peers.write() = TrustStore::new();
        assert!(classified_queue.lock().pop_front_volatile().is_some());

        assert_eq!(
            waiting.await.expect("send_wait task should complete"),
            AdmissionOutcome::Dropped {
                reason: DropReason::UntrustedSender
            },
            "capacity wait must not carry stale classification-time trust into admission"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        assert_eq!(
            classified_queue.lock().entries.len(),
            0,
            "revoked ingress must not be queued after capacity opens"
        );
    }

    #[tokio::test]
    async fn test_classified_send_wait_cannot_miss_capacity_notify_between_check_and_await() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(1, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
            notify,
            classification_context: Some(ctx),
            classified_queue: Some(classified_queue.clone()),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        assert_eq!(
            sender.send_classified(InboxItem::External {
                envelope: make_response_envelope(Uuid::new_v4()),
            }),
            AdmissionOutcome::Admitted
        );

        let released_capacity = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hook_queue = classified_queue.clone();
        let hook_released_capacity = released_capacity.clone();
        let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if !hook_released_capacity.swap(true, Ordering::SeqCst) {
                assert!(
                    hook_queue.lock().pop_front_volatile().is_some(),
                    "test hook must free the exactly-full queue"
                );
            }
        });

        let waiting_sender = sender.clone();
        let waiting = tokio::spawn(CLASSIFIED_SEND_WAIT_BEFORE_AWAIT_HOOK.scope(
            hook,
            async move {
                waiting_sender
                    .send_wait(InboxItem::External {
                        envelope: make_response_envelope(Uuid::new_v4()),
                    })
                    .await
            },
        ));

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .expect("send_wait must observe the capacity wake registered before awaiting")
                .expect("send_wait task should join"),
            AdmissionOutcome::Admitted
        );
        assert!(
            released_capacity.load(Ordering::SeqCst),
            "test hook must have exercised the full-queue wait path"
        );
        assert_eq!(
            classified_queue.lock().entries.len(),
            1,
            "waiting send should be admitted after the raced capacity wake"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_classified_send_wait_wakes_when_queue_closes_while_full() {
        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), false);
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(1, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
            notify,
            classification_context: Some(ctx),
            classified_queue: Some(classified_queue.clone()),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        assert_eq!(
            sender.send_classified(InboxItem::External {
                envelope: make_response_envelope(Uuid::new_v4()),
            }),
            AdmissionOutcome::Admitted
        );

        let closed_queue = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hook_queue = classified_queue.clone();
        let hook_closed_queue = closed_queue.clone();
        let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if !hook_closed_queue.swap(true, Ordering::SeqCst) {
                hook_queue.lock().close();
            }
        });

        let waiting_sender = sender.clone();
        let waiting = tokio::spawn(CLASSIFIED_SEND_WAIT_BEFORE_AWAIT_HOOK.scope(
            hook,
            async move {
                waiting_sender
                    .send_wait(InboxItem::External {
                        envelope: make_response_envelope(Uuid::new_v4()),
                    })
                    .await
            },
        ));

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .expect("send_wait must wake when a full queue is closed")
                .expect("send_wait task should join"),
            AdmissionOutcome::Dropped {
                reason: DropReason::SessionClosed
            }
        );
        assert!(
            closed_queue.load(Ordering::SeqCst),
            "test hook must have exercised the full-queue close path"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_classified_send_wait_stress_backpressures_burst_without_drops() {
        const CAPACITY: usize = 8;
        const WAITERS: usize = 256;

        let sender_pubkey = PubKey::new([1u8; 32]);
        let ctx = make_classification_context(make_trusted("peer", &sender_pubkey), true);
        let actionable_notify = Arc::new(Notify::new());
        let notify = Arc::new(Notify::new());
        let queue = make_classified_queue(CAPACITY, &ctx);
        let counter = queue.dropped_counter();
        let classified_queue = Arc::new(Mutex::new(queue));
        let sender = InboxSender {
            notify,
            classification_context: Some(ctx),
            classified_queue: Some(classified_queue.clone()),
            actionable_notify: Some(actionable_notify),
            dropped_count: Some(counter.clone()),
        };

        for _ in 0..CAPACITY {
            assert_eq!(
                sender.send_classified(InboxItem::External {
                    envelope: make_response_envelope(Uuid::new_v4()),
                }),
                AdmissionOutcome::Admitted
            );
        }

        let mut waiters = Vec::with_capacity(WAITERS);
        for _ in 0..WAITERS {
            let waiting_sender = sender.clone();
            waiters.push(tokio::spawn(async move {
                waiting_sender
                    .send_wait(InboxItem::External {
                        envelope: make_response_envelope(Uuid::new_v4()),
                    })
                    .await
            }));
        }
        tokio::task::yield_now().await;
        assert!(
            waiters.iter().any(|waiter| !waiter.is_finished()),
            "burst should saturate the tiny classified queue and park send_wait tasks"
        );

        for _ in 0..(CAPACITY + WAITERS) {
            loop {
                if classified_queue.lock().pop_front_volatile().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            tokio::task::yield_now().await;
        }

        for waiter in waiters {
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
                    .await
                    .expect("send_wait task should complete without stalling")
                    .expect("send_wait task should join"),
                AdmissionOutcome::Admitted
            );
        }
        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "backpressured burst must not record capacity drops"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_raw_connection_ingress_rejects_untrusted_bridge_without_machine_authority() {
        // Raw transport ingress is not a classification authority. Without the
        // classified runtime/machine path, it cannot infer supervisor-bridge
        // auth exemption from a local compatibility path.
        let receiver = crate::identity::Keypair::generate();
        let sender = crate::identity::Keypair::generate();
        let (_inbox, inbox_sender) = Inbox::new_transport_only();

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender.public_key(),
            to: receiver.public_key(),
            kind: MessageKind::Request {
                objective_id: None,
                content_taint: None,
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        envelope.sign(&sender);

        let outcome = inbox_sender.send_connection_ingress(envelope, true);
        assert_eq!(
            outcome,
            AdmissionOutcome::Dropped {
                reason: DropReason::ClassificationRejected
            }
        );
    }
}
