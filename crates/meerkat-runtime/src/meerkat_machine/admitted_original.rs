//! Cold recovery of one governed original's admission for its source owner.
//!
//! A source owner can lose its in-process record of an admission it was
//! awaiting (a restart between native acceptance and its own custody
//! settlement) while the runtime durably accepted the input.
//! [`MeerkatMachine::with_admitted_original`] lets that owner publish its own
//! custody of exactly that original from the runtime's historical admission,
//! on one exact current registration and live actor, without submitting,
//! replaying or executing anything.
//!
//! The observation is the runtime's own row, read under the registration's
//! mutation gate and the session driver lock (or, for an input already
//! settled and archived out of the live state, its durable witness, read
//! under the gate), and handed to the owner's synchronous publication step
//! while both locks are still held. Nothing of it outlives that call: there
//! is no receipt, registry, stored observation or reconstructed actor, and it
//! confers no operation authority. Historical admission is not active work,
//! completion, permission to resume, or permission to submit missing or
//! uncertain work again.
//!
//! Custody. The call establishes it itself through the publication owner the
//! machine retains for the attachment: it takes that owner's turn boundary B
//! (the endpoint the runtime loop holds across each turn) and has the same
//! owner pin, under B, the controller client its actor actually selected.
//! The caller supplies no boundary, guard or controller; it names only the
//! actor it observed the attachment serving. The owner is replaced only under
//! M (actor-only recovery), so both registry reads check that it is still the
//! same owner allocation, naming that same live actor. A selection equality
//! or a session id is never a substitute, and a replaced actor is refused,
//! never silently succeeded.
//!
//! Lock order. B is taken first, while no lock of this machine is held, and
//! kept until the publication step returns. The call then takes the
//! registration's own mutation gate (M), reads the session registry briefly,
//! reads the durable witness, takes the session driver lock (D), and
//! re-checks the registry without waiting before running the publication
//! step. So the order is B -> M -> registry -> D, as in every other native
//! same-session mutation; B and M span the durable read. The caller must hold
//! no turn boundary of the session. The publication step runs inside B, M
//! and D and must not call back into this machine or its session.
//!
//! Durable read. [`MeerkatMachine::durable_admitted_original`] reads the same
//! historical admission from the session's durable runtime store alone, for a
//! source owner whose session has no current registration, attachment or
//! live actor on this machine (for example after the session was retired).
//! It derives the same qualified key and accepts only the same exact
//! admission, takes no boundary or gate, never consults or revives live
//! state, and returns owned facts that authorize nothing.

use std::sync::Arc;

use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::lifecycle::{CoreExecutorActorIdentity, CoreExecutorPublicationHandle, RunId};
use meerkat_core::{ControllerModelClient, InputId, SessionId};

use super::{
    MeerkatMachine, RuntimeExecutorAttachmentWitness, RuntimeLoopAttachmentSlot,
    RuntimeSessionEntry, RuntimeSessionRegistrationWitness,
};
use crate::identifiers::IdempotencyKey;
use crate::input::Input;
use crate::input_authority::RetainedInputAuthority;
use crate::input_state::{InputLifecycleState, InputState, InputTerminalOutcome};
use crate::traits::RuntimeDriverError;

/// Why no admitted original was observed. No refusal publishes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmittedOriginalRefusal {
    /// The registration, its gate, the live actor attachment or durable
    /// custody cannot be shown current now: a witness of another machine, a
    /// replaced, retiring or degraded registration, a replaced or detached
    /// attachment, an attachment that does not serve exactly the observed
    /// live actor, an attachment owner that cannot take the turn boundary or
    /// pin its actor's controller under it, or contention at the final
    /// check. Observe again.
    Unavailable,
    /// The current registration has no admission under the expected
    /// original's qualified key.
    NotAdmitted,
    /// An admission exists under that key but is not the expected original's
    /// exact one: the expected input is not a keyed governed prompt, the
    /// row's own contributor is missing, duplicated or names another
    /// association, its prompt witness is absent or differs from the expected
    /// input, or the controller the actor pinned does not have the
    /// association's selection.
    Conflict,
}

/// One governed original's historical admission, observed on an exact
/// current registration and live actor, inside
/// [`MeerkatMachine::with_admitted_original`] only. It cannot be stored,
/// cloned or sent, and authorizes nothing: it reports what the runtime
/// recorded, including the input's actual lifecycle phase, for the source
/// owner's own custody decision.
pub struct AdmittedOriginal<'a> {
    registration: &'a RuntimeSessionRegistrationWitness,
    input_id: &'a InputId,
    association: &'a InputAuthorityAssociation,
    phase: InputLifecycleState,
    controller: &'a ControllerModelClient,
}

impl std::fmt::Debug for AdmittedOriginal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmittedOriginal")
            .field("session_id", self.registration.session_id())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl AdmittedOriginal<'_> {
    pub fn session_id(&self) -> &SessionId {
        self.registration.session_id()
    }

    /// The admitted input.
    pub fn input_id(&self) -> &InputId {
        self.input_id
    }

    /// The exact association the runtime retained for its own admission.
    pub fn association(&self) -> &InputAuthorityAssociation {
        self.association
    }

    /// The input's actual lifecycle phase now, as the runtime owns it.
    /// Queued, applied, consumed, superseded, coalesced and abandoned stay
    /// distinct; none of them is a completion or a resume permission.
    pub fn phase(&self) -> InputLifecycleState {
        self.phase
    }

    /// The controller client the attachment's actor actually selected,
    /// pinned by its owner under the turn boundary this call holds; its
    /// selection is the association's own.
    pub fn controller(&self) -> &ControllerModelClient {
        self.controller
    }
}

/// Why [`MeerkatMachine::durable_admitted_original`] read no admission. No
/// refusal reads anything live, revives or changes anything, and none is
/// evidence that the original was not admitted elsewhere or may be submitted
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DurableAdmittedOriginalRefusal {
    /// The expected input is not a keyed governed prompt, or the durable row
    /// under its qualified key is not its exact admission: the row's own
    /// contributor is missing, duplicated or names another association, or
    /// its prompt witness is absent or differs from the expected input.
    Conflict,
    /// The session's durable runtime exists and its store-owned key index
    /// holds no row under the expected original's qualified key: absence in
    /// this runtime, as its store proves it at the time of the read.
    NotAdmitted,
    /// This machine's store holds no durable runtime for the session.
    UnknownRuntime,
    /// The durable evidence cannot be read now: the machine has no persistent
    /// store, the store keeps no exact key index or cannot prove its index
    /// for the runtime, or a read failed. Never a statement of absence.
    Unavailable,
}

/// One governed original's admission as the durable runtime store of its
/// session records it, read by [`MeerkatMachine::durable_admitted_original`].
///
/// Owned and inert: it reports the committed row's facts and authorizes
/// nothing. It carries no payload, result, controller or custody, and none
/// of its facts is a completion, a resume permission or permission to submit
/// the original again.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DurableAdmittedOriginal {
    session_id: SessionId,
    input_id: InputId,
    association: InputAuthorityAssociation,
    phase: InputLifecycleState,
    terminal_outcome: Option<InputTerminalOutcome>,
    last_run_id: Option<RunId>,
    attempt_count: u32,
}

impl std::fmt::Debug for DurableAdmittedOriginal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableAdmittedOriginal")
            .field("session_id", &self.session_id)
            .field("input_id", &self.input_id)
            .field("phase", &self.phase)
            .field("terminal_outcome", &self.terminal_outcome)
            .finish_non_exhaustive()
    }
}

impl DurableAdmittedOriginal {
    /// The session whose durable runtime recorded the admission.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The admitted input: the runtime's actual id for the original.
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// The exact association the runtime retained for its own admission.
    pub fn association(&self) -> &InputAuthorityAssociation {
        &self.association
    }

    /// The input's lifecycle phase as the store last committed it. On a
    /// session that is still registered it can lag the live phase.
    pub fn phase(&self) -> InputLifecycleState {
        self.phase
    }

    /// The input's committed terminal outcome, once it reached one.
    pub fn terminal_outcome(&self) -> Option<&InputTerminalOutcome> {
        self.terminal_outcome.as_ref()
    }

    /// The run the input last contributed to, as the store recorded it.
    pub fn last_run_id(&self) -> Option<&RunId> {
        self.last_run_id.as_ref()
    }

    /// How many run attempts the store recorded for the input.
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }
}

/// The association and runtime-qualified idempotency key of `expected` when
/// it is a keyed governed prompt: the exact original its source owner
/// retains. `None` for any other input.
fn expected_original(expected: &Input) -> Option<(&InputAuthorityAssociation, IdempotencyKey)> {
    let Input::Prompt(_) = expected else {
        return None;
    };
    let association = expected.header().authority_association.as_ref()?;
    let key = crate::input_authority::qualified_idempotency_key(expected)
        .ok()
        .flatten()?;
    Some((association, key))
}

impl MeerkatMachine {
    /// Run `publish` once, synchronously, on the runtime's historical
    /// admission of the governed original `expected` names, while the exact
    /// `registration`, the live attachment `attachment` and its actor `actor`
    /// are proven current and stay so, under that actor's turn boundary.
    ///
    /// `expected` is the source owner's retained evidence of the original:
    /// the keyed governed prompt it submitted. Its association and raw key
    /// derive the qualified idempotency key here, and the admitted row must
    /// be its exact replay by the canonical replay identity (content and
    /// semantic slots, and the retained authority); presence of a prompt
    /// witness alone is never equality. The row must carry exactly one own
    /// contributor, with exactly that association.
    ///
    /// `actor` is the caller's observation of the live actor `attachment`
    /// serves (for a session service's actor, its witness's executor
    /// identity). The call takes the turn boundary of the attachment's own
    /// publication owner and has that owner pin, under it, the controller
    /// client its actor actually selected; the owner must name exactly
    /// `actor`, live, and the pinned controller's selection must be the
    /// association's. See the module documentation for custody and the lock
    /// order; the caller must hold no turn boundary of the session.
    ///
    /// Between the final check and the return of `publish` nothing awaits
    /// and no guard is released: the turn boundary and the registration's
    /// mutation gate exclude turns, executor publication, actor-only
    /// recovery and registration replacement throughout. The source owner's
    /// own state (for example a cancellation of the original) is its to
    /// recheck inside `publish`, without waiting. `publish` must not call
    /// back into this machine or its session.
    ///
    /// # Errors
    /// [`AdmittedOriginalRefusal`]; `publish` did not run. Its own result is
    /// returned unchanged inside `Ok`.
    pub async fn with_admitted_original<T, E>(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
        attachment: &RuntimeExecutorAttachmentWitness,
        actor: &CoreExecutorActorIdentity,
        expected: &Input,
        publish: impl FnOnce(&AdmittedOriginal<'_>) -> Result<T, E>,
    ) -> Result<Result<T, E>, AdmittedOriginalRefusal> {
        use AdmittedOriginalRefusal::{Conflict, NotAdmitted, Unavailable};
        let (association, key) = expected_original(expected).ok_or(Conflict)?;
        if !registration.belongs_to(self)
            || !attachment.belongs_to(self)
            || attachment.session_id() != registration.session_id()
            || attachment.epoch_id() != registration.epoch_id()
        {
            return Err(Unavailable);
        }
        // The attachment's own publication owner, naming the observed actor;
        // no lock of this machine is held while its boundary is awaited.
        let owner = {
            let sessions = self.sessions.read().await;
            let (_, owner) = serving_entry(
                sessions.get(registration.session_id()),
                registration,
                attachment,
                actor,
            )?;
            Arc::clone(owner)
        };
        // B: that owner's turn boundary, held until `publish` returns.
        let boundary = owner.turn_finalization_boundary().ok_or(Unavailable)?;
        let _turn_boundary = boundary.acquire().await.map_err(|_| Unavailable)?;
        // Under B, the same owner pins the controller its actor selected.
        let controller = owner
            .pin_actor_controller_under_turn_boundary()
            .await
            .map_err(|_| Unavailable)?;
        if association.candidate().controller_model.as_ref() != Some(controller.selection()) {
            return Err(Conflict);
        }
        // M: this exact registration's own mutation gate.
        let gate = registration.registration_gate().ok_or(Unavailable)?;
        let _registration_guard = gate.lock_owned().await;
        let driver = {
            let sessions = self.sessions.read().await;
            let entry = owned_entry(
                sessions.get(registration.session_id()),
                registration,
                attachment,
                actor,
                &owner,
            )?;
            Arc::clone(&entry.driver)
        };
        // A settled input is archived out of the live state with its key
        // binding; its row is the durable witness, read under M and before
        // the driver lock.
        let durable = self
            .durable_input_witness_by_idempotency_key_if_present(registration.session_id(), &key.0)
            .await
            .map_err(|_| Unavailable)?;
        let driver_guard = driver.lock().await;
        // The final check: from here until `publish` returns nothing awaits
        // and no guard is released.
        let sessions = self.sessions.try_read().map_err(|_| Unavailable)?;
        let entry = owned_entry(
            sessions.get(registration.session_id()),
            registration,
            attachment,
            actor,
            &owner,
        )?;
        if !Arc::ptr_eq(&entry.driver, &driver) {
            return Err(Unavailable);
        }
        let runtime = driver_guard.as_driver();
        // The live binding decides a live key; an input still in the live
        // ledger keeps its live row and phase.
        let input_id = runtime
            .input_id_for_idempotency_key(&key.0)
            .or_else(|| durable.as_ref().map(|row| row.state.input_id.clone()))
            .ok_or(NotAdmitted)?;
        let (state, phase) = match runtime.input_state(&input_id) {
            Some(state) => (state, runtime.input_phase(&input_id).ok_or(Unavailable)?),
            None => {
                let row = durable
                    .as_ref()
                    .filter(|row| row.state.input_id == input_id)
                    .ok_or(Unavailable)?;
                (&row.state, row.seed.phase)
            }
        };
        let own = own_admission(state, association, expected)?;
        Ok(publish(&AdmittedOriginal {
            registration,
            input_id: &state.input_id,
            association: own.association(),
            phase,
            controller: &controller,
        }))
    }

    /// Read the historical admission of the governed original `expected`
    /// names on `session_id` from this machine's durable runtime store alone.
    ///
    /// The durable sibling of [`Self::with_admitted_original`], for a source
    /// owner whose session has no current registration, live attachment or
    /// actor on this machine, for example after the session was retired.
    /// `expected` is the owner's retained evidence of the original: the keyed
    /// governed prompt it submitted. Its association and raw key derive the
    /// qualified idempotency key here, exactly as for the live call. The
    /// store-owned key index names the row, and that row must be the
    /// original's exact admission: exactly one own contributor, with exactly
    /// that association, and the exact replay of `expected` by the canonical
    /// replay identity (content and semantic slots, and the retained
    /// authority).
    ///
    /// Durable only: no registration, attachment, actor, turn boundary or
    /// gate is required or taken, live state is never consulted, and nothing
    /// is revived, submitted, replayed, executed or published. On a session
    /// that is still registered the durable facts can lag the live ones; an
    /// owner publishing its custody of a live session uses
    /// [`Self::with_admitted_original`]. The result reports what the store
    /// committed and confers no authority: it is not a completion, a result
    /// release, a resume permission or permission to submit the original
    /// again.
    ///
    /// # Errors
    /// [`DurableAdmittedOriginalRefusal`]: `Conflict` for evidence that is not
    /// the exact admission, `NotAdmitted` when the runtime's index holds no row
    /// under the key, `UnknownRuntime` when the store holds no runtime for the
    /// session, and `Unavailable` when the durable evidence cannot be read.
    pub async fn durable_admitted_original(
        &self,
        session_id: &SessionId,
        expected: &Input,
    ) -> Result<DurableAdmittedOriginal, DurableAdmittedOriginalRefusal> {
        use DurableAdmittedOriginalRefusal::{Conflict, NotAdmitted, Unavailable, UnknownRuntime};
        let (association, key) = expected_original(expected).ok_or(Conflict)?;
        // The store's indexed read, with the lifecycle check that separates a
        // runtime without the key from a runtime the store never held.
        let row = match self
            .durable_session_input_witness_by_idempotency_key(session_id, &key.0)
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return Err(NotAdmitted),
            Err(RuntimeDriverError::NotFound { .. }) => return Err(UnknownRuntime),
            Err(_) => return Err(Unavailable),
        };
        let own = own_admission(&row.state, association, expected).map_err(|_| Conflict)?;
        Ok(DurableAdmittedOriginal {
            session_id: session_id.clone(),
            input_id: row.state.input_id.clone(),
            association: own.association().clone(),
            phase: row.seed.phase,
            terminal_outcome: row.seed.terminal_outcome.clone(),
            last_run_id: row.seed.last_run_id.clone(),
            attempt_count: row.seed.attempt_count,
        })
    }
}

/// The current entry of `registration` when it is durable, not retiring,
/// and served by the committed live attachment `attachment`, with that
/// attachment's retained publication owner when it names exactly the live
/// `actor`.
fn serving_entry<'a>(
    entry: Option<&'a RuntimeSessionEntry>,
    registration: &RuntimeSessionRegistrationWitness,
    attachment: &RuntimeExecutorAttachmentWitness,
    actor: &CoreExecutorActorIdentity,
) -> Result<
    (
        &'a RuntimeSessionEntry,
        &'a Arc<dyn CoreExecutorPublicationHandle>,
    ),
    AdmittedOriginalRefusal,
> {
    let entry = entry.ok_or(AdmittedOriginalRefusal::Unavailable)?;
    let served = matches!(
        &entry.attachment_slot,
        RuntimeLoopAttachmentSlot::Attached(current) if current.id == attachment.attachment_id
    );
    // The attachment's own actor, from the owner the machine replaces only
    // under M; nothing is resolved by session id.
    let owner = entry
        .publication_handle
        .as_ref()
        .ok_or(AdmittedOriginalRefusal::Unavailable)?;
    let serves_actor = owner
        .actor_identity()
        .is_some_and(|serving| serving.is_same_actor(actor));
    if !registration.matches_entry(entry)
        || &entry.epoch_id != registration.epoch_id()
        || entry.require_durability_ready().is_err()
        || entry
            .registration_blocked_by_unregister(registration.session_id())
            .is_some()
        || !entry.generated_executor_registration_active()
        || !entry.attachment_is_live()
        || !served
        || !serves_actor
        || !actor.is_live()
    {
        return Err(AdmittedOriginalRefusal::Unavailable);
    }
    Ok((entry, owner))
}

/// [`serving_entry`] whose attachment owner is still exactly `owner`, the
/// allocation whose turn boundary this call holds.
fn owned_entry<'a>(
    entry: Option<&'a RuntimeSessionEntry>,
    registration: &RuntimeSessionRegistrationWitness,
    attachment: &RuntimeExecutorAttachmentWitness,
    actor: &CoreExecutorActorIdentity,
    owner: &Arc<dyn CoreExecutorPublicationHandle>,
) -> Result<&'a RuntimeSessionEntry, AdmittedOriginalRefusal> {
    let (entry, current) = serving_entry(entry, registration, attachment, actor)?;
    if !Arc::ptr_eq(current, owner) {
        return Err(AdmittedOriginalRefusal::Unavailable);
    }
    Ok(entry)
}

/// The row's one own contributor, when it is exactly `association` and the
/// row is the exact replay of `expected` by the canonical replay identity.
fn own_admission<'s>(
    state: &'s InputState,
    association: &InputAuthorityAssociation,
    expected: &Input,
) -> Result<&'s RetainedInputAuthority, AdmittedOriginalRefusal> {
    let mut own = state
        .authority_contributors
        .iter()
        .filter(|contributor| contributor.input_id() == &state.input_id);
    let (Some(own), None) = (own.next(), own.next()) else {
        return Err(AdmittedOriginalRefusal::Conflict);
    };
    if own.association() != association {
        return Err(AdmittedOriginalRefusal::Conflict);
    }
    crate::input_state::verify_exact_replay(state, expected)
        .map_err(|_| AdmittedOriginalRefusal::Conflict)?;
    Ok(own)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
