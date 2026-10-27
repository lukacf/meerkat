//! Direct Core attempt custody for one live local preparation.
//! No gateway installation or reset policy is supplied here.
use super::*;
use crate::application_setup::resource::{SetupResourceRegistration, SetupResourceSelection};
use async_trait::async_trait;
use meerkat_core::mcp_attachment::McpAttachmentRefusal;
use meerkat_mcp::{
    McpAttemptCancellation, McpAttemptDisposition, McpAttemptGuard, McpAttemptGuardFactory,
    McpAttemptGuardRequest, McpAttemptKey, McpAttemptOpenOutcome, McpAttemptOperation,
    McpAttemptProcessEnd, McpGuardFinishError, McpServerCloseOutcome, McpStdioLeaderOutcome,
};
use std::sync::{Arc, Mutex, Weak};

/// The existing preparation's one owner slot and native guard factory.
/// Install this through Core's McpAttemptGuards before native construction.
/// It never replaces an existing guard installation or configures a gateway.
/// The host must close native transports, join Core's retained guard calls,
/// then recover and clean up this owner before shutting down its runtime.
pub struct LocalRoundAttempts {
    selection: Weak<SetupResourceSelection>,
    state: Arc<Mutex<State>>,
}

struct Registration {
    session: String,
    participant: Arc<SetupResourceRegistration>,
}

struct State {
    owner: Option<LocalRoundOwner>,
    registration: Option<Registration>,
    active: Option<McpAttemptKey>,
    started: bool,
    closed: bool,
}

impl LocalRoundAttempts {
    pub(crate) fn new(owner: LocalRoundOwner, selection: Weak<SetupResourceSelection>) -> Self {
        Self {
            selection,
            state: Arc::new(Mutex::new(State {
                owner: Some(owner),
                registration: None,
                active: None,
                started: false,
                closed: false,
            })),
        }
    }

    /// Fence this factory and take its original owner only when no begin or
    /// guard holds it. None means custody is still elsewhere, never joined.
    /// Retain this factory, close the native transports and await Core's same
    /// begin/finish calls before trying again. A returned owner can still have
    /// an OS-wait failure; retain it and await cancel_pending_round as needed.
    /// This does not inspect or replace Core's bounded completion history.
    pub fn take_closed_owner(&self) -> Option<LocalRoundOwner> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.closed = true;
        if state.active.is_some() {
            return None;
        }
        let mut owner = state.owner.take()?;
        if self.state.is_poisoned() {
            owner.poisoned = true;
        }
        Some(owner)
    }

    // Only the existing selected-destination callback supplies this actual
    // participant. Native retirement, not a copied generation, ends it.
    pub(crate) fn bind_registration(
        &self,
        session: String,
        participant: Arc<SetupResourceRegistration>,
    ) -> Result<(), String> {
        participant.current()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "local round custody unavailable")?;
        if state.closed {
            return Err("local round custody is closed".into());
        }
        if let Some(previous) = &state.registration {
            if previous.participant.current().is_ok() {
                return Err("previous local round registration remains current".into());
            }
        }
        state.registration = Some(Registration {
            session,
            participant,
        });
        Ok(())
    }
}

#[async_trait]
impl McpAttemptGuardFactory for LocalRoundAttempts {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancellation: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        let selection = self
            .selection
            .upgrade()
            .ok_or(McpAttachmentRefusal::Unavailable)?;
        if request.server.name != selection.server().name {
            return Ok(None);
        }
        let helper = selection
            .helper_projection()
            .map_err(|_| McpAttachmentRefusal::Unavailable)?;
        if request.server != *helper.server()
            || request.key.server != helper.server().name
            || request.key.attempt == 0
            || request.owner.member.as_ref() != Some(helper.member())
            || !matches!(
                request.operation,
                McpAttemptOperation::Add | McpAttemptOperation::Reload
            )
        {
            return Err(McpAttachmentRefusal::Mismatch);
        }
        let (mut owner, initial) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| McpAttachmentRefusal::Unavailable)?;
            let registration = state
                .registration
                .as_ref()
                .ok_or(McpAttachmentRefusal::Unavailable)?;
            if registration.session != request.owner.session_id.to_string() {
                return Err(McpAttachmentRefusal::Mismatch);
            }
            registration
                .participant
                .current()
                .map_err(|_| McpAttachmentRefusal::NotCurrent)?;
            if state.closed || state.active.is_some() || cancellation.is_requested() {
                return Err(McpAttachmentRefusal::Unavailable);
            }
            let owner = state
                .owner
                .as_ref()
                .ok_or(McpAttachmentRefusal::Unavailable)?;
            if owner.poisoned || owner.has_pending_round() {
                return Err(McpAttachmentRefusal::Unavailable);
            }
            owner
                .check_resources()
                .map_err(|_| McpAttachmentRefusal::Unavailable)?;
            let initial = !state.started;
            let owner = state
                .owner
                .take()
                .ok_or(McpAttachmentRefusal::Unavailable)?;
            state.started = true;
            state.active = Some(request.key.clone());
            (owner, initial)
        };
        let round = if initial {
            // The fresh preparation needs no successor offer. It still stays
            // poisoned until this exact attempt proves a graceful installed close
            // or a safe initial cancellation. Neither is new native admission.
            owner.poisoned = true;
            Round::Initial(owner)
        } else {
            match owner
                .begin_owned(async move {
                    cancellation.requested().await;
                    Ok(())
                })
                .await
            {
                Ok(guard) => Round::Offered(guard),
                Err((owner, _)) => {
                    restore(&self.state, &request.key, owner, None);
                    return Err(McpAttachmentRefusal::Unavailable);
                }
            }
        };
        Ok(Some(Box::new(Guard {
            state: self.state.clone(),
            key: request.key,
            round,
        })))
    }
}

enum Round {
    Initial(LocalRoundOwner),
    Offered(OwnedLocalRoundGuard),
}
struct Guard {
    state: Arc<Mutex<State>>,
    key: McpAttemptKey,
    round: Round,
}

#[derive(Clone, Copy)]
enum InitialCancellation {
    NotSpawned,
    Closed,
}

fn restore(
    state: &Mutex<State>, key: &McpAttemptKey, mut owner: LocalRoundOwner,
    initial_cancel: Option<InitialCancellation>,
) -> bool {
    // No external callback executes under this lock. On a poisoned lock keep
    // custody, but never authorize reuse. The sole claimant took this slot;
    // neither registration nor close can put another owner into it.
    let mut slot = state.lock().unwrap_or_else(|poison| poison.into_inner());
    let current = !state.is_poisoned() && slot.active.as_ref() == Some(key);
    if !current {
        owner.poisoned = true;
    }
    if current && !owner.poisoned
        && matches!(initial_cancel, Some(InitialCancellation::NotSpawned))
    {
        // Core never spawned this initial descriptor. Return its unused
        // preparation. A future begin still selects and validates its own
        // exact request and current registration.
        slot.started = false;
    }
    slot.owner = Some(owner);
    slot.active = None;
    current
}

fn graceful(disposition: &McpAttemptDisposition, key: &McpAttemptKey, settle: bool) -> bool {
    if !settle
        || &disposition.key != key
        || !matches!(disposition.open, McpAttemptOpenOutcome::Installed { .. })
    {
        return false;
    }
    normal_close(&disposition.process)
}

fn normal_close(process: &McpAttemptProcessEnd) -> bool {
    let McpAttemptProcessEnd::Closed(receipt) = process else {
        return false;
    };
    McpServerCloseOutcome::Stdio(receipt.clone()).is_settled()
        && matches!(&receipt.leader, McpStdioLeaderOutcome::Reaped(status) if status.success())
        && receipt.sigterm.is_none()
        && receipt.sigkill.is_none()
}

fn initial_cancellation(
    disposition: &McpAttemptDisposition, key: &McpAttemptKey,
) -> Option<InitialCancellation> {
    if &disposition.key != key
        || !matches!(disposition.open, McpAttemptOpenOutcome::Cancelled { .. })
    {
        return None;
    }
    match &disposition.process {
        McpAttemptProcessEnd::NotSpawned => Some(InitialCancellation::NotSpawned),
        process if normal_close(process) => Some(InitialCancellation::Closed),
        _ => None,
    }
}

impl Guard {
    async fn finish(
        self,
        disposition: McpAttemptDisposition,
        settle: bool,
    ) -> Result<(), McpGuardFinishError> {
        let healthy = graceful(&disposition, &self.key, settle);
        let initial_cancel = if !settle && matches!(&self.round, Round::Initial(_)) {
            initial_cancellation(&disposition, &self.key)
        } else {
            None
        };
        let reusable = healthy || initial_cancel.is_some();
        let (mut owner, result) = match self.round {
            Round::Initial(mut owner) => {
                let result = if reusable {
                    // Exact native finish returns the retained custody;
                    // it does not authorize another resource use. The next
                    // begin validates the installed resources before taking it.
                    Ok(())
                } else {
                    Err(unavailable())
                };
                if result.is_ok() {
                    owner.poisoned = false;
                }
                (owner, result)
            }
            Round::Offered(guard) => {
                if healthy {
                    guard.settle().await
                } else {
                    guard.cancel().await
                }
            }
        };
        if !reusable {
            owner.poisoned = true;
        }
        // A normally closed initial child can use the existing successor
        // offer. Offered-round cancellation still requires its own protocol
        // settlement and remains poisoned. No session authority is reused.
        // Return custody before telling Core that the consuming finish ended.
        let restored = restore(&self.state, &self.key, owner, initial_cancel);
        if reusable && restored && result.is_ok() {
            Ok(())
        } else {
            Err(McpGuardFinishError::new(
                "The exact local setup attempt did not close and settle its retained round",
            ))
        }
    }
}
#[async_trait]
impl McpAttemptGuard for Guard {
    async fn settle(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, true).await
    }
    async fn cancel(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, false).await
    }
}
