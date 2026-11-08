//! Deliver a completed transfer through the existing G/P native input owner.
//! The live transfer attempt retains one Input, including its generated ID.
//! Store leases, missing rows and departed donors cannot manufacture entry.
use super::*;
use crate::original_ingress::{PendingToolkitOriginal, shared_entry::SharedEntry};
use meerkat_authorization_contracts::evidence::HistoricalEvidenceRef;
use meerkat_core::{SessionId, lifecycle::InputId};
use meerkat_mobkit::identity_first::{AgentAddressability, GovernedAdmission};
use meerkat_runtime::{
    SessionServiceRuntimeExt, identifiers::IdempotencyKey, input_state::verify_exact_replay,
};
use toolkit_store::{EffectClaimKind, EffectIntent, EffectLease, ReplaySafety, StoredEffect};

pub(super) mod historical;
#[cfg(test)]
mod review_custody_tests;
#[cfg(test)]
mod tests;

const RECEIPT_SCHEMA: &str = "toolkit.workflow-context-input";

/// Native input admission only. This neither completes a G turn nor settles
/// any waiter's installation, result or business effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDeliveryObservation {
    Pending,
    /// The previous owned attempt joined without admission. A later call must
    /// acquire fresh G/P and workflow authority before using the same input.
    NotApplied,
    Admitted {
        session_id: String,
        input_id: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryReceipt {
    admission: WorkAdmission,
    membership: SetupWorkflowWaiter,
    session: SessionId,
    input: InputId,
    association: InputAuthorityAssociation,
}

/// Only the existing transfer attempt owns this slot. It is not a durable
/// reconstruction recipe and has no payload getter or serialization.
pub(super) struct DeliverySlot {
    input: Input,
    event: IdempotencyKey,
    workflow: Arc<HeldSetupWorkflow>,
    bound: OnceLock<Input>,
    run: Mutex<Option<Arc<DeliveryRun>>>,
    native: Mutex<Option<Arc<DeliveryNative>>>,
}

pub(super) struct DeliveryRun {
    join: tokio::sync::Mutex<DeliveryJoin>,
    native: Arc<Mutex<Option<Arc<DeliveryNative>>>>,
    joined: AtomicBool,
}
enum DeliveryJoin {
    Running(tokio::task::JoinHandle<Result<WorkflowDeliveryObservation, PrivateOriginalError>>),
    Joined(Result<WorkflowDeliveryObservation, PrivateOriginalError>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeDecision {
    NeverSubmitted,
    Submitted,
    NotAdmitted,
    Accepted,
}

struct DeliveryNative {
    // Only an observed successful Dispatch claim can support nonadmission.
    // A lost claim response or a competing owner's lease cannot fill this.
    claim: OnceLock<StoredEffect>,
    // Only this owner's successful NotApplied acknowledgment enables retry.
    // Generic Pending also follows RetrySafe and is not sufficient.
    not_applied: OnceLock<StoredEffect>,
    decision: Mutex<NativeDecision>,
    joined: AtomicBool,
    custody: Arc<DeliveryCustody>,
}
#[derive(Default)]
struct DeliveryCustody {
    pending: Mutex<Option<PendingToolkitOriginal>>,
    unresolved: Mutex<Vec<Box<dyn GovernedSettlement>>>,
    accepted: OnceLock<GovernedAdmission>,
    entry: OnceLock<Arc<SharedEntry>>,
    // Removable custody of this exact delivery attempt, not its retained
    // settlement evidence. Unknown admission keeps it for later recovery.
    review: Mutex<Option<ReviewCustody>>,
}

impl DeliveryCustody {
    fn review_custody(&self) -> Result<ReviewCustody, PrivateOriginalError> {
        Ok(self
            .review
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .clone()
            .unwrap_or_default())
    }

    fn release_settled_review(&self) -> Result<(), PrivateOriginalError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let unresolved = self
            .unresolved
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if pending.is_some()
            || !unresolved.is_empty()
            || self.entry.get().is_some_and(|entry| !entry.settled())
        {
            return Ok(());
        }
        let review = self
            .review
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .take();
        drop(unresolved);
        drop(pending);
        // Core may notify observers on the last drop. No owner lock is held.
        drop(review);
        Ok(())
    }
}

impl InstalledWorkflowContext {
    /// At most one owned attempt per call. Caller loss leaves the actual task
    /// in the transfer's recovery owner. Warm delivery reuses its retained
    /// input; a restored disclosure can only acknowledge exact native history.
    pub async fn deliver(&self) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        self.deliver_with_review(ReviewCustody::default()).await
    }

    pub(crate) async fn deliver_with_review(
        &self,
        review_custody: ReviewCustody,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let retained_review = self.attempt.delivery_review()?;
        review_custody
            .retain_io(retained_review.retain_io(async {
                // This origin can acknowledge history only, even through deliver().
                // Native absence never turns a recovered disclosure into a new input.
                if !self.attempt.locally_entered() {
                    return self
                        .transfer
                        .reconcile_delivery_with_review(review_custody.clone())
                        .await;
                }
                let source = self
                    .transfer
                    .source
                    .upgrade()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let slot = self
                    .transfer
                    .with_recipient(&source, |workflow| {
                        if !Arc::ptr_eq(&workflow, &self.workflow) {
                            return Err(PrivateOriginalError::Conflict);
                        }
                        let mut held = self
                            .attempt
                            .delivery
                            .lock()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                        if let Some(slot) = held.as_ref() {
                            return Ok(slot.clone());
                        }
                        let (input, event) = delivery_input(&self.attempt)?;
                        let slot = Arc::new(DeliverySlot {
                            input,
                            event,
                            workflow,
                            bound: OnceLock::new(),
                            run: Mutex::new(None),
                            native: Mutex::new(None),
                        });
                        *held = Some(slot.clone());
                        Ok(slot)
                    })
                    .await?;
                slot.execute(
                    source,
                    self.transfer.clone(),
                    self.attempt.clone(),
                    true,
                    review_custody.clone(),
                )
                .await
            }))
            .await
    }
}

impl InstalledWorkflowContextTransfer {
    /// Metadata-only reconciliation of an entered delivery or an already
    /// restored committed disclosure. It may acknowledge an exact accepted
    /// input after recipient closure, but never exposes payload or submits.
    /// Historical absence cannot establish nonadmission or retry permission.
    pub async fn reconcile_delivery(
        self: &Arc<Self>,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        self.reconcile_delivery_with_review(ReviewCustody::default())
            .await
    }

    pub(super) async fn reconcile_delivery_with_review(
        self: &Arc<Self>,
        review_custody: ReviewCustody,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let retained_review = attempt_review(self)?;
        review_custody
            .retain_io(retained_review.retain_io(async {
                let source = self
                    .source
                    .upgrade()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let retained = self.canonical()?;
                let attempt = retained
                    .attempt
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .clone();
                let Some(attempt) = attempt else {
                    return Ok(WorkflowDeliveryObservation::Pending);
                };
                if !attempt.locally_entered() {
                    return historical::reconcile(
                        source,
                        retained,
                        attempt,
                        review_custody.clone(),
                    )
                    .await;
                }
                let slot = attempt
                    .delivery
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .clone();
                let Some(slot) = slot else {
                    return Ok(WorkflowDeliveryObservation::Pending);
                };
                // The private retained handle identifies the existing host owner. No
                // A-current check, open-recipient check or new native authorization is
                // needed to record that its exact old input was already admitted.
                source
                    .host_lifetime
                    .get()
                    .and_then(Weak::upgrade)
                    .ok_or(PrivateOriginalError::Unavailable)?
                    .validate_current_selection()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                slot.execute(source, retained, attempt, false, review_custody.clone())
                    .await
            }))
            .await
    }
}

impl DeliverySlot {
    pub(super) fn review_custody(&self) -> Result<ReviewCustody, PrivateOriginalError> {
        let native = self
            .native
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .clone();
        match native {
            Some(native) => native.custody.review_custody(),
            None => Ok(ReviewCustody::default()),
        }
    }

    async fn execute(
        self: &Arc<Self>,
        source: Arc<PrivateOriginals>,
        transfer: Arc<InstalledWorkflowContextTransfer>,
        attempt: Arc<TransferAttempt>,
        may_dispatch: bool,
        review_custody: ReviewCustody,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let control = attempt
            .recovery
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let run = control
            .enter_transfer(|| {
                let mut current = self
                    .run
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                let slot = self.clone();
                let task_review = review_custody.clone();
                Ok::<_, PrivateOriginalError>(DeliveryRun::retain_or_start(
                    &mut current,
                    move |task_native| async move {
                        // This entered task retains its originating custody through
                        // effect acknowledgement; a reused run keeps its own hold.
                        let review_custody = task_review;
                        let retained_review = slot.review_custody()?;
                        review_custody
                            .retain_io(retained_review.retain_io(async {
                                if may_dispatch {
                                    slot.drive(
                                        &source,
                                        &transfer,
                                        &attempt,
                                        &task_native,
                                        &review_custody,
                                    )
                                    .await
                                } else {
                                    let effect = slot.effect(&source, &attempt).await?;
                                    slot.reconcile(&source, &attempt, &effect).await
                                }
                            }))
                            .await
                    },
                ))
            })
            .ok_or(PrivateOriginalError::Unavailable)??;
        run.join().await
    }

    async fn effect(
        &self,
        source: &PrivateOriginals,
        attempt: &TransferAttempt,
    ) -> Result<StoredEffect, PrivateOriginalError> {
        delivery_effect(source, attempt).await
    }

    fn receipt(&self, attempt: &TransferAttempt) -> Result<RecordData, PrivateOriginalError> {
        delivery_receipt(
            attempt,
            &self.workflow.session,
            self.input.id(),
            self.workflow.planning.association(),
        )
    }

    fn admitted(&self) -> WorkflowDeliveryObservation {
        WorkflowDeliveryObservation::Admitted {
            session_id: self.workflow.session.to_string(),
            input_id: self.input.id().to_string(),
        }
    }

    async fn drive(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        transfer: &Arc<InstalledWorkflowContextTransfer>,
        attempt: &Arc<TransferAttempt>,
        owned_native: &Mutex<Option<Arc<DeliveryNative>>>,
        review_custody: &ReviewCustody,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let planning = transfer.planning()?;
        let effect = self.effect(source, attempt).await?;
        if !delivery_claim_ready(&self.native, &effect)? {
            return self.reconcile(source, attempt, &effect).await;
        }
        transfer
            .with_recipient(source, |workflow| {
                if !Arc::ptr_eq(&workflow, &self.workflow) {
                    return Err(PrivateOriginalError::Conflict);
                }
                Ok(())
            })
            .await?;
        let snapshot = source
            .workflow_store()?
            .load_setup_workflow(&transfer.selected.workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if snapshot.terms() != &planning.terms {
            return Err(PrivateOriginalError::Conflict);
        }
        // Validates exact historical membership; a committed donor may now be
        // departed. The final fence chooses any still-live recipient interest.
        let _ = snapshot
            .departed(&attempt.envelope.membership)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let Some((native, lease)) = claim_delivery_effect_with_review(
            source.store.as_ref(),
            &effect,
            snapshot.revision(),
            &self.native,
            owned_native,
            review_custody.clone(),
        )
        .await?
        else {
            return Ok(WorkflowDeliveryObservation::Pending);
        };
        let route = source.route(&transfer.selected.planning_source.route)?;
        let controller = source
            .owner
            .workflow_controller(&self.workflow.planning)
            .map_err(custody_error)?;
        let binder = review_custody.bind(Arc::new(DeliveryBinder {
            owner: source.owner.clone(),
            original: self.workflow.planning.clone(),
            slot: self.clone(),
            native: native.clone(),
            controller,
            authentication: evidence(
                &route.authentication_domain,
                &planning.submission.transport_authentication,
            )?,
            planning,
        }));
        let runtime = source.bound_identity_runtime()?;
        let identity = runtime
            .owned_identity_for_member_alias(&self.workflow.member.member)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        *native
            .decision
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)? = NativeDecision::Submitted;
        let result = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some(&self.workflow.member.member),
                self.input.clone(),
                binder,
            )
            .await;
        let observation = match result {
            Ok(outcome) => {
                if outcome.admission.session_id != self.workflow.session
                    || outcome.admission.input_id != *self.input.id()
                {
                    return Err(PrivateOriginalError::Conflict);
                }
                *native
                    .decision
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)? = NativeDecision::Accepted;
                // Native dedup is possible only for this exact retained Input.
                // It never promotes the retry; exact readback settles below.
                if outcome.settlement.is_ok()
                    && !outcome.admission.deduplicated
                    && native.custody.accepted.get() == Some(&outcome.admission)
                {
                    self.admitted()
                } else {
                    WorkflowDeliveryObservation::Pending
                }
            }
            Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Bind(_)
                | GovernedInputError::Mob(_)
                | GovernedInputError::EntryRefused(_),
            )) => {
                *native
                    .decision
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)? = NativeDecision::NotAdmitted;
                // Selected Meta has joined its definite nonadmission decision
                // and withdrawn its settlement before returning these arms.
                native
                    .custody
                    .pending
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .take();
                WorkflowDeliveryObservation::Pending
            }
            Err(_) => WorkflowDeliveryObservation::Pending,
        };
        let receipt = self.receipt(attempt)?;
        let outcome = if matches!(observation, WorkflowDeliveryObservation::Admitted { .. }) {
            EffectOutcome::Succeeded { receipt }
        } else {
            EffectOutcome::Uncertain { evidence: receipt }
        };
        let resolved = source
            .store
            .acknowledge_effect(&lease, outcome.clone())
            .await
            .map_err(store_error)?;
        if matches!(observation, WorkflowDeliveryObservation::Admitted { .. })
            && native.owns_claim(&resolved)
            && resolved.status == EffectStatus::Succeeded
            && resolved.lease.is_none()
            && resolved.outcome.as_ref() == Some(&outcome)
        {
            native.custody.release_settled_review()?;
        }
        Ok(observation)
    }

    async fn reconcile(
        &self,
        source: &PrivateOriginals,
        attempt: &TransferAttempt,
        effect: &StoredEffect,
    ) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let receipt = self.receipt(attempt)?;
        if effect.status == EffectStatus::Succeeded {
            if effect.outcome.as_ref() != Some(&EffectOutcome::Succeeded { receipt }) {
                return Err(PrivateOriginalError::Conflict);
            }
            let native = self
                .native
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .clone();
            if let Some(native) = native
                && native.joined.load(Ordering::Acquire)
                && native.owns_claim(effect)
            {
                native.custody.release_settled_review()?;
            }
            return Ok(self.admitted());
        }
        if effect.attempt == 0 || effect.status == EffectStatus::Failed {
            return Ok(WorkflowDeliveryObservation::Pending);
        }
        let native = self
            .native
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .clone();
        let Some(native) = native else {
            return Ok(WorkflowDeliveryObservation::Pending);
        };
        // A missing row is conclusive only after the actual dispatch owner
        // joined and only when it never submitted or got definite nonadmission.
        if !native.joined.load(Ordering::Acquire) {
            return Ok(WorkflowDeliveryObservation::Pending);
        }
        let row = source
            .machine()?
            .input_state(&self.workflow.session, self.input.id())
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let applied = if let Some(row) = row {
            let bound = self.bound.get().ok_or(PrivateOriginalError::Conflict)?;
            if row.state.input_id != *self.input.id()
                || verify_exact_replay(&row.state, bound).is_err()
            {
                return Err(PrivateOriginalError::Conflict);
            }
            // The retained G source is already Admitted. This settles only
            // this input's pending=false binding, without resetting that source.
            if let Some(pending) = native
                .custody
                .pending
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .take()
            {
                pending.admit().map_err(custody_error)?;
            }
            native
                .custody
                .unresolved
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .clear();
            if let Some(entry) = native.custody.entry.get() {
                entry.settle().map_err(custody_error)?;
            }
            // Exact admitted input evidence is historical: it can settle
            // this same delivery after closure without a negative claim witness.
            true
        } else {
            // Absence belongs to this dispatch attempt only if its actual
            // claim was received and still names the effect being reconciled.
            // No claim response, a losing claim, or an older attempt is unknown.
            if !native.can_reconcile_absence_for(effect)? {
                return Ok(WorkflowDeliveryObservation::Pending);
            }
            false
        };
        let claim = match source
            .store
            .claim_reconciliation(&effect.key, "installed-context-observe", 60_000)
            .await
        {
            Ok(claim) => claim,
            Err(StoreError::Claimed | StoreError::EffectState | StoreError::Conflict) => {
                return Ok(if applied {
                    self.admitted()
                } else {
                    WorkflowDeliveryObservation::Pending
                });
            }
            Err(error) => return Err(store_error(error)),
        };
        let lease = exact_reconciliation_lease(effect, &claim)?;
        let resolved = source
            .store
            .reconcile_effect(
                lease,
                if applied {
                    Reconciliation::Applied { receipt }
                } else {
                    Reconciliation::NotApplied { evidence: receipt }
                },
            )
            .await
            .map_err(store_error)?;
        if applied {
            if native.owns_claim(&resolved)
                && resolved.status == EffectStatus::Succeeded
                && resolved.lease.is_none()
                && resolved.outcome.as_ref()
                    == Some(&EffectOutcome::Succeeded {
                        receipt: self.receipt(attempt)?,
                    })
            {
                native.custody.release_settled_review()?;
            }
            Ok(self.admitted())
        } else {
            native.record_not_applied(resolved)?;
            native.custody.release_settled_review()?;
            Ok(WorkflowDeliveryObservation::NotApplied)
        }
    }

    pub(super) async fn join(&self, finished_only: bool) -> Result<(), PrivateOriginalError> {
        let (run, poisoned) = match self.run.lock() {
            Ok(held) => (held.clone(), false),
            Err(error) => (error.into_inner().clone(), true),
        };
        if let Some(run) = run
            && (!finished_only || run.finished())
        {
            let _ = run.join().await;
        }
        if poisoned {
            Err(PrivateOriginalError::Unavailable)
        } else {
            Ok(())
        }
    }

    // Caller has completed full joins and checked the actual registration gate
    // is closed. Ordinary join/Pending/JoinError alone cannot call this path.
    pub(super) async fn finalize_closed_review(&self) -> Result<(), PrivateOriginalError> {
        let run = self
            .run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(run) = run {
            let joined = run.join.lock().await;
            if !matches!(&*joined, DeliveryJoin::Joined(_)) {
                return Err(PrivateOriginalError::Unavailable);
            }
            drop(joined);
        }
        let native = self
            .native
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let review = if let Some(native) = native {
            native
                .custody
                .review
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        } else {
            None
        };
        // The stored outcome, unresolved settlements and native source remain
        // unchanged. Core observers run only after every owner lock is gone.
        drop(review);
        Ok(())
    }
}

/// One producer representation for both live submission and exact historical
/// queries. Its fresh ID/time are never acceptance evidence; Core excludes
/// those two fields from replay and returns the actual admitted InputId.
fn delivery_input(
    attempt: &TransferAttempt,
) -> Result<(Input, IdempotencyKey), PrivateOriginalError> {
    delivery_input_for(&attempt.envelope, &attempt.admission)
}
fn delivery_input_for(
    envelope: &TransferEnvelope,
    admission: &WorkAdmission,
) -> Result<(Input, IdempotencyKey), PrivateOriginalError> {
    let bytes = canonical_json(envelope).map_err(|_| PrivateOriginalError::Conflict)?;
    let input = Input::Prompt(PromptInput::new(
        String::from_utf8(bytes).map_err(|_| PrivateOriginalError::Conflict)?,
        None,
    ));
    let key = envelope
        .membership
        .delivery_effect()
        .map_err(|_| PrivateOriginalError::Conflict)?;
    let event = IdempotencyKey::new(format!(
        "workflow-context-{}",
        digest(&canonical_json(&(&key, admission)).map_err(|_| PrivateOriginalError::Conflict)?)
    ));
    Ok((input, event))
}

async fn delivery_effect(
    source: &PrivateOriginals,
    attempt: &TransferAttempt,
) -> Result<StoredEffect, PrivateOriginalError> {
    delivery_effect_for(source, &attempt.envelope.membership).await
}
async fn delivery_effect_for(
    source: &PrivateOriginals,
    membership: &SetupWorkflowWaiter,
) -> Result<StoredEffect, PrivateOriginalError> {
    let key = membership
        .delivery_effect()
        .map_err(|_| PrivateOriginalError::Conflict)?;
    let request = RecordData {
        schema: "meerkat_toolkit.setup_workflow.delivery".into(),
        version: 1,
        body: serde_json::json!({"workflow": membership.workflow(), "payload": membership}),
    };
    let expected = EffectIntent {
        id: key.id.clone(),
        kind: "setup_workflow_delivery".into(),
        idempotency_key: ContentDigest::of_bytes(
            &canonical_json(&request).map_err(|_| PrivateOriginalError::Conflict)?,
        )
        .to_string(),
        request,
        replay_safety: ReplaySafety::ReconcileBeforeRetry,
    };
    let effect = source
        .store
        .get_effect(&key)
        .await
        .map_err(store_error)?
        .ok_or(PrivateOriginalError::Unavailable)?;
    if effect.key != key
        || effect.intent != expected
        || effect.request_digest != expected.digest().map_err(store_error)?
    {
        return Err(PrivateOriginalError::Conflict);
    }
    Ok(effect)
}

fn delivery_receipt(
    attempt: &TransferAttempt,
    session: &SessionId,
    input: &InputId,
    association: &InputAuthorityAssociation,
) -> Result<RecordData, PrivateOriginalError> {
    delivery_receipt_for(
        &attempt.envelope.membership,
        &attempt.admission,
        session,
        input,
        association,
    )
}
fn delivery_receipt_for(
    membership: &SetupWorkflowWaiter,
    admission: &WorkAdmission,
    session: &SessionId,
    input: &InputId,
    association: &InputAuthorityAssociation,
) -> Result<RecordData, PrivateOriginalError> {
    Ok(RecordData {
        schema: RECEIPT_SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(DeliveryReceipt {
            admission: admission.clone(),
            membership: membership.clone(),
            session: session.clone(),
            input: input.clone(),
            association: association.clone(),
        })
        .map_err(|_| PrivateOriginalError::Conflict)?,
    })
}

/// Canonical Pending alone cannot grant a retry: RetrySafe can also produce it.
/// The exact joined native attempt must retain our successful NotApplied reply.
fn delivery_claim_ready(
    retained: &Mutex<Option<Arc<DeliveryNative>>>,
    effect: &StoredEffect,
) -> Result<bool, PrivateOriginalError> {
    if effect.status != EffectStatus::Pending {
        return Ok(false);
    }
    if effect.attempt == 0 {
        return Ok(true);
    }
    let native = retained
        .lock()
        .map_err(|_| PrivateOriginalError::Unavailable)?
        .clone();
    match native {
        Some(native) => native.can_retry(effect),
        None => Ok(false),
    }
}

/// The retained DeliveryRun serializes this boundary with every observation.
/// Only a definite record-CAS Conflict permits restoring the previous witness;
/// every ambiguous or competing claim leaves the new unowned witness in place.
#[cfg(test)]
async fn claim_delivery_effect(
    store: &dyn toolkit_store::ToolkitStore,
    effect: &StoredEffect,
    record_revision: u64,
    retained: &Mutex<Option<Arc<DeliveryNative>>>,
    owned: &Mutex<Option<Arc<DeliveryNative>>>,
) -> Result<Option<(Arc<DeliveryNative>, EffectLease)>, PrivateOriginalError> {
    claim_delivery_effect_with_review(
        store,
        effect,
        record_revision,
        retained,
        owned,
        ReviewCustody::default(),
    )
    .await
}

async fn claim_delivery_effect_with_review(
    store: &dyn toolkit_store::ToolkitStore,
    effect: &StoredEffect,
    record_revision: u64,
    retained: &Mutex<Option<Arc<DeliveryNative>>>,
    owned: &Mutex<Option<Arc<DeliveryNative>>>,
    review_custody: ReviewCustody,
) -> Result<Option<(Arc<DeliveryNative>, EffectLease)>, PrivateOriginalError> {
    if !delivery_claim_ready(retained, effect)? {
        return Ok(None);
    }
    let prior = retained
        .lock()
        .map_err(|_| PrivateOriginalError::Unavailable)?
        .clone();
    let native = Arc::new(DeliveryNative {
        claim: OnceLock::new(),
        not_applied: OnceLock::new(),
        decision: Mutex::new(NativeDecision::NeverSubmitted),
        joined: AtomicBool::new(false),
        custody: Arc::new(DeliveryCustody {
            review: Mutex::new(Some(review_custody)),
            ..DeliveryCustody::default()
        }),
    });
    *owned
        .lock()
        .map_err(|_| PrivateOriginalError::Unavailable)? = Some(native.clone());
    *retained
        .lock()
        .map_err(|_| PrivateOriginalError::Unavailable)? = Some(native.clone());
    // Install unknown custody before the await, including a committed claim
    // whose response is lost. The old proof cannot settle that new attempt.
    let claim = match store
        .claim_effect(
            &effect.key,
            record_revision,
            "installed-context-delivery",
            60_000,
        )
        .await
    {
        Ok(EffectClaimOutcome::Claimed(claim)) => claim,
        Err(StoreError::Conflict) => {
            // The store's record revision check precedes every effect write.
            // Restore exact custody, never a free retry flag. A later drive
            // rereads the effect and rejects a newer or changed attempt.
            *retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)? = prior;
            // This exact pre-write CAS refusal never reached native dispatch.
            native.custody.release_settled_review()?;
            return Ok(None);
        }
        Ok(EffectClaimOutcome::NeedsReconciliation(_))
        | Err(StoreError::Claimed | StoreError::EffectState) => return Ok(None),
        Err(error) => return Err(store_error(error)),
    };
    let lease = native.record_claim(effect, claim)?;
    Ok(Some((native, lease)))
}

impl DeliveryNative {
    fn record_not_applied(&self, resolved: StoredEffect) -> Result<(), PrivateOriginalError> {
        if resolved.status != EffectStatus::Pending
            || resolved.lease.is_some()
            || resolved.outcome.is_some()
            || !self.can_reconcile_absence_for(&resolved)?
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // Repeated exact observation can acknowledge the same attempt again.
        // Preserve the first successful reply; its timestamps are not authority.
        self.not_applied.get_or_init(|| resolved);
        Ok(())
    }

    fn can_retry(&self, effect: &StoredEffect) -> Result<bool, PrivateOriginalError> {
        let exact = self.not_applied.get().is_some_and(|resolved| {
            resolved.key == effect.key
                && resolved.intent == effect.intent
                && resolved.request_digest == effect.request_digest
                && resolved.attempt == effect.attempt
        });
        Ok(effect.status == EffectStatus::Pending
            && exact
            && self.can_reconcile_absence_for(effect)?)
    }

    fn record_claim(
        &self,
        prior: &StoredEffect,
        claimed: StoredEffect,
    ) -> Result<EffectLease, PrivateOriginalError> {
        if prior.status != EffectStatus::Pending
            || prior.attempt.checked_add(1) != Some(claimed.attempt)
            || claimed.key != prior.key
            || claimed.intent != prior.intent
            || claimed.request_digest != prior.request_digest
            || claimed.request_digest != claimed.intent.digest().map_err(store_error)?
            || claimed.status != EffectStatus::InFlight
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let lease = claimed
            .lease
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if lease.kind != EffectClaimKind::Dispatch || lease.key != claimed.key {
            return Err(PrivateOriginalError::Conflict);
        }
        let lease = lease.clone();
        self.claim
            .set(claimed)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        Ok(lease)
    }

    // Native absence is a separate required read in the caller. This checks
    // only the joined dispatch attempt that can support that negative read.
    fn can_reconcile_absence_for(
        &self,
        effect: &StoredEffect,
    ) -> Result<bool, PrivateOriginalError> {
        if !self.joined.load(Ordering::Acquire) || !self.owns_claim(effect) {
            return Ok(false);
        }
        let decision = *self
            .decision
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(matches!(
            decision,
            NativeDecision::NeverSubmitted | NativeDecision::NotAdmitted
        ) && self
            .custody
            .unresolved
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .is_empty()
            && self
                .custody
                .pending
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .is_none()
            && self.custody.entry.get().is_none_or(|entry| entry.settled()))
    }

    fn owns_claim(&self, effect: &StoredEffect) -> bool {
        self.claim.get().is_some_and(|claim| {
            claim.key == effect.key
                && claim.intent == effect.intent
                && claim.request_digest == effect.request_digest
                && claim.attempt == effect.attempt
        })
    }
}

/// The native observation belongs to the exact effect attempt read before the
/// claim await. A lease fences the returned attempt, not an earlier observation.
/// This is shared by positive and negative reconciliation; it grants no retry.
fn exact_reconciliation_lease<'a>(
    observed: &StoredEffect,
    claimed: &'a StoredEffect,
) -> Result<&'a EffectLease, PrivateOriginalError> {
    if observed.attempt == 0
        || claimed.attempt != observed.attempt
        || claimed.key != observed.key
        || claimed.intent != observed.intent
        || claimed.request_digest != observed.request_digest
        || claimed.request_digest != claimed.intent.digest().map_err(store_error)?
        || claimed.status != EffectStatus::Uncertain
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let lease = claimed
        .lease
        .as_ref()
        .ok_or(PrivateOriginalError::Unavailable)?;
    if lease.kind != EffectClaimKind::Reconcile || lease.key != claimed.key {
        return Err(PrivateOriginalError::Conflict);
    }
    // The store validates its own lease fence and expiry at the actual write.
    Ok(lease)
}

impl DeliveryRun {
    /// Register the automatic advance before its first async observation.
    /// This reuses the transfer's slot and the existing recovery stop boundary.
    pub(super) fn register_advance<F, Fut>(
        control: &RecoveryControl,
        current: &Mutex<Option<Arc<Self>>>,
        start: F,
    ) -> Result<Arc<Self>, PrivateOriginalError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<WorkflowDeliveryObservation, PrivateOriginalError>>
            + Send
            + 'static,
    {
        control
            .enter_transfer(|| {
                let mut current = current
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                Ok(Self::retain_or_start(&mut current, move |_| start()))
            })
            .ok_or(PrivateOriginalError::Unavailable)?
    }

    fn retain_or_start<F, Fut>(current: &mut Option<Arc<Self>>, start: F) -> Arc<Self>
    where
        F: FnOnce(Arc<Mutex<Option<Arc<DeliveryNative>>>>) -> Fut,
        Fut: std::future::Future<Output = Result<WorkflowDeliveryObservation, PrivateOriginalError>>
            + Send
            + 'static,
    {
        // This runs under the slot mutex and the recovery stop mutex. Dispatch
        // and metadata observation cannot outlive their serialization and
        // later reconcile an old witness against a newer effect attempt.
        if let Some(prior) = current.as_ref()
            && !prior.joined.load(Ordering::Acquire)
        {
            return prior.clone();
        }
        let native = Arc::new(Mutex::new(None));
        let join = tokio::spawn(start(native.clone()));
        let run = Arc::new(Self {
            join: tokio::sync::Mutex::new(DeliveryJoin::Running(join)),
            native,
            joined: AtomicBool::new(false),
        });
        *current = Some(run.clone());
        run
    }

    pub(super) fn finished(&self) -> bool {
        self.join.try_lock().is_ok_and(|held| match &*held {
            DeliveryJoin::Running(task) => task.is_finished(),
            DeliveryJoin::Joined(_) => true,
        })
    }
    pub(super) async fn join(&self) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
        let mut held = self.join.lock().await;
        let result = match &mut *held {
            DeliveryJoin::Running(task) => {
                task.await.unwrap_or(Err(PrivateOriginalError::Unavailable))
            }
            DeliveryJoin::Joined(result) => result.clone(),
        };
        *held = DeliveryJoin::Joined(result.clone());
        if let Some(native) = self
            .native
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .as_ref()
        {
            native.joined.store(true, Ordering::Release);
        }
        self.joined.store(true, Ordering::Release);
        result
    }
}

struct DeliveryBinder {
    owner: Arc<ToolkitOriginalOwner>,
    original: Arc<RetainedToolkitOriginal>,
    slot: Arc<DeliverySlot>,
    native: Arc<DeliveryNative>,
    controller: ControllerModelClient,
    authentication: HistoricalEvidenceRef,
    planning: Arc<InstalledWorkflowPlanning>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for DeliveryBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let incoming = context
            .controller
            .plain_facts()
            .map_err(|_| "delivery controller unavailable")?
            .route_key();
        let retained = self
            .controller
            .plain_facts()
            .map_err(|_| "delivery controller unavailable")?
            .route_key();
        if context.session_id != self.slot.workflow.session
            || incoming != retained
            || context.spec.addressability != AgentAddressability::InternalOnly
            || input.id() != self.slot.input.id()
        {
            return Err("delivery destination changed".into());
        }
        let (input, pending) = self
            .owner
            .bind_authenticated_input(
                &self.original,
                &context.session_id,
                input,
                self.slot.event.clone(),
                self.authentication.clone(),
                context.controller.clone(),
            )
            .map_err(|_| "delivery binding refused")?;
        let canonical = self.slot.bound.get_or_init(|| input.clone());
        if canonical_json(canonical).map_err(|_| "delivery input encoding refused")?
            != canonical_json(&input).map_err(|_| "delivery input encoding refused")?
        {
            return Err("delivery input binding changed".into());
        }
        let mut custody = self
            .native
            .custody
            .pending
            .lock()
            .map_err(|_| "delivery custody unavailable")?;
        if custody.is_some() {
            return Err("delivery binder was already entered".into());
        }
        *custody = Some(pending);
        drop(custody);
        Ok(BoundGovernedInput {
            input,
            settlement: Box::new(DeliverySettlement {
                custody: Arc::downgrade(&self.native.custody),
                session: context.session_id.clone(),
                input: self.slot.input.id().clone(),
            }),
            entry_fence: Some(Arc::new(DeliveryFence {
                planning: self.planning.clone(),
                custody: self.native.custody.clone(),
            })),
        })
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _: &str) {
        // The same attempt's removable review was installed before claim and
        // remains through this transfer and later pending/entry/effect recovery.
        // Poison does not permit custody loss or a later nonadmission claim.
        self.native
            .custody
            .unresolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(settlement);
    }
}
struct DeliverySettlement {
    custody: Weak<DeliveryCustody>,
    session: SessionId,
    input: InputId,
}
impl GovernedSettlement for DeliverySettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        if admission.deduplicated
            || admission.session_id != self.session
            || admission.input_id != self.input
        {
            return Err("delivery acknowledgement changed".into());
        }
        let custody = self
            .custody
            .upgrade()
            .ok_or("delivery custody unavailable")?;
        let pending = custody
            .pending
            .lock()
            .map_err(|_| "delivery custody unavailable")?
            .take()
            .ok_or("delivery custody already settled")?;
        pending
            .admit()
            .map_err(|_| "delivery source settlement failed")?;
        custody
            .accepted
            .set(admission.clone())
            .map_err(|_| "delivery acknowledged twice".into())
    }
}

/// Same final local source-interest boundary as first launch, with this exact
/// attempt's entry retained for later native reconciliation. Never calls the
/// async recipient observer or reenters the native runtime from Meta's fence.
struct DeliveryFence {
    planning: Arc<InstalledWorkflowPlanning>,
    custody: Arc<DeliveryCustody>,
}
impl GovernedEntryFence for DeliveryFence {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        let source = self.planning.source.upgrade().ok_or_else(|| {
            GovernedEntryRefusal::Unavailable("delivery source unavailable".into())
        })?;
        let mut interests = self.planning.interests.lock().map_err(|_| {
            GovernedEntryRefusal::Unavailable("delivery interests unavailable".into())
        })?;
        let now = source
            .owner
            .clock()
            .now()
            .map_err(|_| GovernedEntryRefusal::Unavailable("delivery clock unavailable".into()))?
            .unix_ms;
        if now >= self.planning.terms.deadline_ms {
            return Err(GovernedEntryRefusal::Expired);
        }
        let mut unavailable = false;
        for interest in interests.iter_mut() {
            if interest.closed || now >= interest.membership.deadline_ms() {
                continue;
            }
            let Some(original) = &interest.original else {
                continue;
            };
            match source.owner.enter_workflow_launch(original) {
                Ok(entry) => {
                    if self.custody.entry.set(entry.clone()).is_err() {
                        let _ = entry.settle();
                        return Err(GovernedEntryRefusal::Unavailable(
                            "delivery already entered".into(),
                        ));
                    }
                    interest.entries.push(entry.clone());
                    return Ok(Box::new(DeliveryPermit { entry }));
                }
                Err(crate::original_ingress::OriginalWriterError::OperationCancelled) => continue,
                Err(_) => unavailable = true,
            }
        }
        if unavailable {
            Err(GovernedEntryRefusal::Unavailable(
                "delivery waiter unavailable".into(),
            ))
        } else {
            Err(GovernedEntryRefusal::Retired)
        }
    }
}
struct DeliveryPermit {
    entry: Arc<SharedEntry>,
}
impl GovernedEntryPermit for DeliveryPermit {
    fn finish(self: Box<Self>, outcome: GovernedEntryOutcome) {
        if outcome != GovernedEntryOutcome::Uncertain {
            let _ = self.entry.settle();
        }
    }
}
