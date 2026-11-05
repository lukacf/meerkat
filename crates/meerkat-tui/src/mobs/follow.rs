//! Following helpers: every helper the customizer saw spawning is
//! subscribed through its mob (from its earliest retained event, so a fast
//! helper's whole run is seen), its session is bound in the registry (for
//! nesting), and its events go to the UI scoped to it until it leaves its
//! mob. A retired member's session stream does not end by itself, so the
//! follower also checks the mob's roster ([`ROSTER_CHECK`]).

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use meerkat_core::EventSourceIdentity;
use meerkat_core::comms::{EventStream, SessionEventCursor};
use meerkat_mob::{AgentIdentity, MobError, MobHandle, MobId};
use meerkat_mob_mcp::MobMcpState;
use tokio::sync::mpsc;

use super::customizer::Spawning;
use super::registry::HelperRegistry;

/// Attempts to subscribe to a helper that is not in its mob's roster yet.
const SUBSCRIBE_ATTEMPTS: u32 = 400;

/// Pause between subscription attempts.
const SUBSCRIBE_RETRY: Duration = Duration::from_millis(5);

/// How often a follower checks that its helper is still in its mob.
const ROSTER_CHECK: Duration = Duration::from_millis(200);

/// Follow every helper announced on `spawning` until the channel closes.
pub async fn watch(
    state: Weak<MobMcpState>,
    registry: Arc<HelperRegistry>,
    mut spawning: mpsc::UnboundedReceiver<Spawning>,
) {
    while let Some(helper) = spawning.recv().await {
        tokio::spawn(follow(Weak::clone(&state), Arc::clone(&registry), helper));
    }
}

/// Forward one helper's events until it leaves its mob.
async fn follow(state: Weak<MobMcpState>, registry: Arc<HelperRegistry>, helper: Spawning) {
    let Spawning { key, tag } = helper;
    let mob_id = MobId::from(key.0.clone());
    let identity = AgentIdentity::from(key.1.clone());
    match subscribe(&state, &mob_id, &identity).await {
        Ok((handle, mut stream, session)) => {
            if let Some(session) = session {
                registry.bind_session(session, &tag);
            }
            let mut check = tokio::time::interval(ROSTER_CHECK);
            check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    envelope = stream.next() => {
                        let Some(envelope) = envelope else { break };
                        if let EventSourceIdentity::Session { session_id } = &envelope.source {
                            registry.bind_session(session_id.clone(), &tag);
                        }
                        registry.emit(&tag, envelope.payload);
                    }
                    _ = check.tick() => {
                        if !is_member(&handle, &identity).await {
                            // Deliver what it published before it left.
                            while let Some(Some(envelope)) = futures::FutureExt::now_or_never(stream.next()) {
                                registry.emit(&tag, envelope.payload);
                            }
                            break;
                        }
                    }
                }
            }
        }
        Err(err) => {
            tracing::warn!(helper = %identity, mob = %mob_id, error = %err, "cannot follow helper");
        }
    }
    registry.finished(&key);
}

/// Whether `identity` is still a member of `handle`'s mob (retiring counts
/// as gone).
async fn is_member(handle: &MobHandle, identity: &AgentIdentity) -> bool {
    handle
        .list_members()
        .await
        .iter()
        .any(|member| &member.agent_identity == identity)
}

/// Subscribe to `identity` in `mob_id` from its earliest retained event,
/// retrying while the mob or the member is not registered yet. Also returns
/// the mob's handle and the member's session when the mob names it.
async fn subscribe(
    state: &Weak<MobMcpState>,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<(MobHandle, EventStream, Option<meerkat_core::SessionId>), MobError> {
    let mut last = MobError::Internal("helper never appeared in its mob".to_string());
    for _ in 0..SUBSCRIBE_ATTEMPTS {
        let state = state
            .upgrade()
            .ok_or_else(|| MobError::Internal("the mob host is gone".to_string()))?;
        match state.handle_for(mob_id).await {
            Ok(handle) => match handle
                .subscribe_agent_events_from(identity, SessionEventCursor::Earliest)
                .await
            {
                Ok(subscription) => {
                    let session = handle.resolve_bridge_session_id(identity).await;
                    return Ok((handle, subscription.stream, session));
                }
                Err(err) => last = err,
            },
            Err(err) => last = err,
        }
        drop(state);
        tokio::time::sleep(SUBSCRIBE_RETRY).await;
    }
    Err(last)
}
