//! Single flight for the live open's durable-source body load.
//!
//! The actor validates a member's roster and session binding inline for
//! `ValidateLiveDurableSourceAvailability`, then loads the durable source body
//! off the actor loop. That load reads, decodes, and hashes the whole session
//! body, so it can take seconds on a large member. Without sharing, every
//! concurrent open started its own load, and a caller that timed out left its
//! load running while the next caller started another.
//!
//! This registry keeps at most one body load per session in flight. A caller
//! that arrives while a load runs waits on that load's result instead of
//! starting a second one; a caller that goes away leaves the one load running
//! for the others. A load still running at [`LIVE_DURABLE_SOURCE_LOAD_CEILING`]
//! is abandoned as temporarily unavailable, and its slot is released so a
//! stalled store cannot pin every later open to one hung read.
//!
//! This is actor-owned shell state for read sharing. It holds no MobMachine
//! fact: the roster and binding checks stay inline in the actor.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::types::SessionId;
use tokio::sync::{oneshot, watch};

use super::super::LiveBridgeOperationStartError;
use super::super::session_service::MobSessionService;

/// Longest one shared durable-source body load is driven before it is
/// abandoned as temporarily unavailable.
pub(in crate::runtime) const LIVE_DURABLE_SOURCE_LOAD_CEILING: Duration = Duration::from_secs(30);

type LoadResult = Result<(), LiveBridgeOperationStartError>;
type LoadSlot = watch::Receiver<Option<LoadResult>>;

/// Durable-source body loads in flight, at most one per session.
#[derive(Clone, Default)]
pub(in crate::runtime) struct LiveDurableSourceLoads(
    Arc<std::sync::Mutex<HashMap<SessionId, LoadSlot>>>,
);

impl LiveDurableSourceLoads {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, LoadSlot>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many body loads are running.
    #[cfg(test)]
    pub(in crate::runtime) fn in_flight(&self) -> usize {
        self.lock().len()
    }

    /// Answer `reply_tx` from the one body load of `session_id` in flight,
    /// starting that load when none is running. Never blocks the caller.
    pub(in crate::runtime) fn validate(
        &self,
        session_service: Arc<dyn MobSessionService>,
        session_id: SessionId,
        reply_tx: oneshot::Sender<LoadResult>,
    ) {
        let (mut result_rx, owner) = {
            let mut loads = self.lock();
            if let Some(slot) = loads.get(&session_id) {
                (slot.clone(), None)
            } else {
                let (result_tx, result_rx) = watch::channel(None);
                loads.insert(session_id.clone(), result_rx.clone());
                (result_rx, Some(result_tx))
            }
        };
        if let Some(result_tx) = owner {
            let slot = LoadSlotRelease {
                loads: self.clone(),
                session_id: session_id.clone(),
                result_rx: result_rx.clone(),
            };
            tokio::spawn(async move {
                let result = tokio::time::timeout(
                    LIVE_DURABLE_SOURCE_LOAD_CEILING,
                    load_durable_source(session_service.as_ref(), &slot.session_id),
                )
                .await
                .unwrap_or_else(|_elapsed| {
                    tracing::warn!(
                        session_id = %slot.session_id,
                        ceiling_ms = LIVE_DURABLE_SOURCE_LOAD_CEILING.as_millis() as u64,
                        "live durable-source body load abandoned at its ceiling"
                    );
                    Err(LiveBridgeOperationStartError::TemporarilyUnavailable)
                });
                // Release the slot before publishing: every caller that
                // joined already holds the result channel, and a caller that
                // arrives after this load settled starts a fresh one rather
                // than reading a finished result.
                drop(slot);
                result_tx.send_replace(Some(result));
            });
        }
        tokio::spawn(async move {
            let result = match result_rx.wait_for(Option::is_some).await {
                Ok(result) => result
                    .clone()
                    .unwrap_or(Err(LiveBridgeOperationStartError::Unavailable)),
                // The owning task went away without a result (actor exit).
                Err(_) => Err(LiveBridgeOperationStartError::Unavailable),
            };
            let _ = reply_tx.send(result);
        });
    }
}

/// Release of one load's registry slot, on completion or when the owning task
/// is dropped. A successor load's slot is never removed.
struct LoadSlotRelease {
    loads: LiveDurableSourceLoads,
    session_id: SessionId,
    result_rx: LoadSlot,
}

impl Drop for LoadSlotRelease {
    fn drop(&mut self) {
        let mut loads = self.loads.lock();
        if loads
            .get(&self.session_id)
            .is_some_and(|slot| slot.same_channel(&self.result_rx))
        {
            loads.remove(&self.session_id);
        }
    }
}

/// The durable-source check itself: the exact session must load.
async fn load_durable_source(
    session_service: &dyn MobSessionService,
    session_id: &SessionId,
) -> LoadResult {
    let source = session_service
        .load_persisted_session(session_id)
        .await
        .map_err(|_| LiveBridgeOperationStartError::Rejected)?;
    if source
        .as_ref()
        .is_none_or(|session| session.id() != session_id)
    {
        return Err(LiveBridgeOperationStartError::Rejected);
    }
    Ok(())
}
