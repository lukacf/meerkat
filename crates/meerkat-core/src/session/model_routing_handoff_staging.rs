//! Run-local staging for a model-requested permanent routing handoff.
//!
//! # Why staging exists at all
//!
//! A run that asks for a permanent model change cannot perform that change
//! itself. The provider identity the run is executing against is fixed for the
//! duration of that run: rebinding it mid-run would mean one transcript was
//! produced by two different models with no committed boundary between them,
//! and the tool call that asked for the change would be answered by a model
//! that never saw the request.
//!
//! So the tool stages. Staging is deliberately the weakest possible act:
//! in-memory, run-scoped, invisible to every other owner, and discarded by
//! default. Nothing durable happens here and nothing routes off this state.
//!
//! # The exactly-one promotion rule
//!
//! Staged intent becomes a committed
//! [`SessionModelRoutingControlRecord::ModelRoutingIntentRequested`] only when
//! the originating run reaches a clean terminal boundary, and only immediately
//! before that run's ordinary final checkpoint. Every other ending — failure,
//! interruption, cancellation, hook denial, extraction error, a callback still
//! pending — drops the slot with the run and commits nothing.
//!
//! That asymmetry is the whole safety argument. A run that did not finish did
//! not earn the right to redirect the next one, and an intent that was never
//! committed cannot be discovered later by a bootstrap or a pre-dequeue seam,
//! because those read the committed session and nothing else.
//!
//! # What this module is NOT
//!
//! * It is not routing authority. No provider call consults it.
//! * It is not durable state. It never crosses a process boundary.
//! * It is not a queue. At most one intent is staged per run, by construction.
//! * It is not the lifecycle owner. Claim, realization, conflict adjudication,
//!   and terminality belong to generated machine authority reading the
//!   *committed* log.
//!
//! # Idempotence and conflict
//!
//! Restating the same target within one run is idempotent and returns the
//! already-minted request identity, so a model that repeats itself does not
//! mint two requests for one decision. Naming a *different* target after one is
//! staged is a typed conflict rather than a silent overwrite: the run has
//! already been told its first choice was accepted, and quietly replacing it
//! would make the tool's own answer a lie.

use std::sync::Mutex;

use crate::image_generation::{
    SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin, SwitchTurnReasonTextDisposition,
    SwitchTurnRequestId,
};
use crate::lifecycle::identifiers::RunId;
use crate::lifecycle::run_primitive::ModelId;
use crate::session::model_routing_control::{
    ModelRoutingControlAppendError, SessionModelRoutingControlRecord,
};

/// One run's staged permanent routing intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedModelRoutingHandoff {
    request_id: SwitchTurnRequestId,
    intent: SwitchTurnIntent,
}

impl StagedModelRoutingHandoff {
    #[must_use]
    pub const fn request_id(&self) -> &SwitchTurnRequestId {
        &self.request_id
    }

    #[must_use]
    pub const fn intent(&self) -> &SwitchTurnIntent {
        &self.intent
    }

    #[must_use]
    pub const fn target_model(&self) -> &ModelId {
        &self.intent.target_model
    }

    /// Bind this staged intent to the exact run that is committing it.
    ///
    /// The run id is supplied by the committing caller rather than captured at
    /// staging time, so a record can never claim a run that did not actually
    /// reach a clean boundary carrying it.
    pub fn into_committed_request(
        self,
        originating_run_id: RunId,
    ) -> Result<SessionModelRoutingControlRecord, ModelRoutingControlAppendError> {
        SessionModelRoutingControlRecord::request(self.request_id, originating_run_id, self.intent)
    }
}

/// Outcome of staging one target within a run.
///
/// Deliberately NOT `#[non_exhaustive]`: the slot holds at most one intent, so
/// "minted it" and "it was already exactly this" is the complete space. Adding
/// a third outcome should break every consumer at compile time rather than be
/// absorbed by a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRoutingHandoffStageOutcome {
    /// The slot was empty and now holds this request.
    Staged { request_id: SwitchTurnRequestId },
    /// The exact same target was already staged by this run.
    AlreadyStaged { request_id: SwitchTurnRequestId },
}

impl ModelRoutingHandoffStageOutcome {
    #[must_use]
    pub const fn request_id(&self) -> &SwitchTurnRequestId {
        match self {
            Self::Staged { request_id } | Self::AlreadyStaged { request_id } => request_id,
        }
    }

    /// Whether this call is the one that minted the staged request.
    #[must_use]
    pub const fn is_newly_staged(&self) -> bool {
        matches!(self, Self::Staged { .. })
    }
}

/// Refusals raised while staging.
///
/// Closed for the same reason as the outcome: a caller must map every failure
/// mode onto a deliberate tool-error class, and a wildcard would let a future
/// variant silently inherit whichever class happened to be the fallback.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelRoutingHandoffStageError {
    /// A different target is already staged by this run.
    #[error(
        "this run already staged a permanent switch to '{staged_target}'; '{requested_target}' conflicts with it"
    )]
    ConflictingTarget {
        staged_target: ModelId,
        requested_target: ModelId,
    },
    /// The staging slot's lock was poisoned by a panicking holder.
    ///
    /// Represented rather than recovered: the slot's invariant is "at most one
    /// intent, minted exactly once", and a panic mid-mutation cannot prove that
    /// still holds.
    #[error("model-routing handoff staging slot is unusable")]
    SlotUnusable,
}

/// Run-scoped slot holding at most one staged permanent routing intent.
///
/// Shared between the staging tool and the agent that owns promotion. The
/// agent clears it at each run boundary, so an intent staged by one run can
/// never be promoted by another.
#[derive(Debug, Default)]
pub struct ModelRoutingHandoffStagingSlot {
    staged: Mutex<Option<StagedModelRoutingHandoff>>,
}

impl ModelRoutingHandoffStagingSlot {
    #[must_use]
    pub fn new() -> Self {
        Self {
            staged: Mutex::new(None),
        }
    }

    /// Stage `target_model` as this run's permanent routing intent.
    ///
    /// `request_id` is supplied by the caller so the identity is deterministic
    /// under test and minted by exactly one owner in production.
    pub fn stage(
        &self,
        request_id: SwitchTurnRequestId,
        target_model: ModelId,
    ) -> Result<ModelRoutingHandoffStageOutcome, ModelRoutingHandoffStageError> {
        let mut staged = self
            .staged
            .lock()
            .map_err(|_| ModelRoutingHandoffStageError::SlotUnusable)?;
        match staged.as_ref() {
            Some(existing) if existing.target_model() == &target_model => {
                Ok(ModelRoutingHandoffStageOutcome::AlreadyStaged {
                    request_id: existing.request_id,
                })
            }
            Some(existing) => Err(ModelRoutingHandoffStageError::ConflictingTarget {
                staged_target: existing.target_model().clone(),
                requested_target: target_model,
            }),
            None => {
                *staged = Some(StagedModelRoutingHandoff {
                    request_id,
                    intent: model_origin_until_changed_intent(target_model),
                });
                Ok(ModelRoutingHandoffStageOutcome::Staged { request_id })
            }
        }
    }

    /// Read the staged intent without consuming it.
    pub fn peek(&self) -> Result<Option<StagedModelRoutingHandoff>, ModelRoutingHandoffStageError> {
        self.staged
            .lock()
            .map(|staged| staged.clone())
            .map_err(|_| ModelRoutingHandoffStageError::SlotUnusable)
    }

    /// Consume the staged intent, leaving the slot empty.
    ///
    /// Taking is the only read the promotion path uses, so one staged intent
    /// cannot be promoted twice even if a caller re-enters the boundary.
    pub fn take(&self) -> Result<Option<StagedModelRoutingHandoff>, ModelRoutingHandoffStageError> {
        self.staged
            .lock()
            .map(|mut staged| staged.take())
            .map_err(|_| ModelRoutingHandoffStageError::SlotUnusable)
    }

    /// Discard anything staged, without promoting it.
    ///
    /// Poisoning is intentionally not an error here: clearing is what every
    /// non-committing ending does, and a run boundary must never be blocked
    /// from discarding uncommitted intent.
    pub fn clear(&self) {
        let mut staged = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *staged = None;
    }
}

/// The one intent shape a staged handoff may take.
///
/// `UntilChanged` + `Model` origin is exactly what
/// `model_routing_control::intent_is_durable_handoff` admits, so a staged
/// intent is representable as a committed record by construction rather than
/// by a later validation that could drift from it.
fn model_origin_until_changed_intent(target_model: ModelId) -> SwitchTurnIntent {
    SwitchTurnIntent {
        target_model,
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::session::model_routing_control::intent_is_durable_handoff;

    fn request_id(byte: u8) -> SwitchTurnRequestId {
        SwitchTurnRequestId::new(uuid::Uuid::from_bytes([byte; 16]))
    }

    #[test]
    fn staged_intent_is_a_representable_durable_handoff() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        slot.stage(request_id(1), ModelId::new("model-a"))
            .expect("first stage");
        let staged = slot.peek().expect("slot readable").expect("staged intent");
        assert!(intent_is_durable_handoff(staged.intent()));
    }

    #[test]
    fn restating_the_same_target_reuses_the_minted_request() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        let first = slot
            .stage(request_id(2), ModelId::new("model-a"))
            .expect("first stage");
        let second = slot
            .stage(request_id(3), ModelId::new("model-a"))
            .expect("duplicate stage");
        assert!(first.is_newly_staged());
        assert!(!second.is_newly_staged());
        assert_eq!(first.request_id(), second.request_id());
        assert_eq!(second.request_id(), &request_id(2));
    }

    #[test]
    fn a_second_distinct_target_conflicts_instead_of_overwriting() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        slot.stage(request_id(4), ModelId::new("model-a"))
            .expect("first stage");
        let error = slot
            .stage(request_id(5), ModelId::new("model-b"))
            .expect_err("conflicting target");
        assert_eq!(
            error,
            ModelRoutingHandoffStageError::ConflictingTarget {
                staged_target: ModelId::new("model-a"),
                requested_target: ModelId::new("model-b"),
            }
        );
        let staged = slot.peek().expect("slot readable").expect("staged intent");
        assert_eq!(staged.target_model(), &ModelId::new("model-a"));
    }

    #[test]
    fn taking_the_slot_leaves_nothing_to_promote_twice() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        slot.stage(request_id(6), ModelId::new("model-a"))
            .expect("stage");
        assert!(slot.take().expect("first take").is_some());
        assert!(slot.take().expect("second take").is_none());
    }

    #[test]
    fn clearing_discards_uncommitted_intent() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        slot.stage(request_id(7), ModelId::new("model-a"))
            .expect("stage");
        slot.clear();
        assert!(slot.peek().expect("slot readable").is_none());
    }

    #[test]
    fn committed_request_binds_the_run_supplied_at_promotion() {
        let slot = ModelRoutingHandoffStagingSlot::new();
        slot.stage(request_id(8), ModelId::new("model-a"))
            .expect("stage");
        let staged = slot.take().expect("take").expect("staged intent");
        let run_id = RunId::new();
        let record = staged
            .into_committed_request(run_id.clone())
            .expect("representable request");
        assert_eq!(record.request_id(), &request_id(8));
        assert_eq!(record.originating_run_id(), &run_id);
        assert_eq!(record.intent().target_model, ModelId::new("model-a"));
    }
}
