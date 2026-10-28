//! The auth binding a live actor runs, checked before it is reused.
//!
//! A member is rebuilt on the auth binding its current spec names. Where a
//! path would keep a session's live actor instead of rebuilding it (a
//! Resume-mode spawn of a session with a live actor, a restore that keeps a
//! live session, a wake onto a live successor session, a durability reload
//! whose build found the session live), the actor is kept only when it runs
//! that binding: a live actor is never reused on another binding. Where a
//! restore, a wake or a reload finds the conflict, the member is recorded
//! broken and the live actor is retired, so no surface reaches it.

use meerkat_core::types::SessionId;

use super::session_service::MobSessionService;
use crate::error::MobError;
use crate::ids::AgentIdentity;

/// Whether the live actor of `session_id` runs `requested`, the auth binding
/// the spec of `member` names: the binding its session recorded (persisted
/// when the actor was built, before it served) is `requested` as named, or
/// the form a build of `requested` records (stamped with the realm that
/// declares it, as the session service resolves it). Anything else, a
/// session that recorded no binding included, is
/// [`MobError::MemberAuthBindingConflict`]; a metadata read that fails is
/// its error. A requested binding the service cannot resolve has no other
/// form, so only its named form can match.
pub(super) async fn verify_live_actor_auth_binding(
    session_service: &dyn MobSessionService,
    member: &AgentIdentity,
    session_id: &SessionId,
    requested: &meerkat_core::AuthBindingRef,
) -> Result<(), MobError> {
    let recorded = session_service
        .load_persisted_session_metadata(session_id)
        .await
        .map_err(MobError::SessionError)?
        .and_then(|view| view.session_metadata)
        .and_then(|metadata| metadata.auth_binding);
    if recorded.as_ref() == Some(requested) {
        return Ok(());
    }
    if let Some(recorded_binding) = recorded.as_ref()
        && session_service
            .recorded_auth_binding(requested)
            .await
            .ok()
            .flatten()
            .as_ref()
            == Some(recorded_binding)
    {
        return Ok(());
    }
    Err(MobError::MemberAuthBindingConflict {
        member_id: member.clone(),
        session_id: session_id.clone(),
        requested: Box::new(requested.clone()),
        recorded: recorded.map(Box::new),
    })
}

/// Retire the live actor of `session_id` that a binding conflict refused to
/// keep (at a restore, a wake or a reload), so a member recorded broken
/// keeps no live actor on the old binding that a surface could reach. Fail
/// closed: the session's exact current runtime registration is unregistered
/// to terminal, interrupting any work in flight, and the session's live
/// actor is then discarded (a session already without one is retired). A
/// build without the runtime adapter has no runtime registration to end, so
/// it only discards the live actor.
pub(super) async fn retire_conflicting_live_actor(
    session_service: &dyn MobSessionService,
    session_id: &SessionId,
) -> Result<(), MobError> {
    #[cfg(feature = "runtime-adapter")]
    {
        let adapter = session_service
            .acquire_runtime_adapter(None)
            .map_err(|error| {
                MobError::Internal(format!(
                    "retiring the conflicting live actor of session {session_id}: {error}"
                ))
            })?;
        if let Some(adapter) = adapter
            && let Some(registration) = adapter
                .current_session_registration_witness(session_id)
                .await
        {
            adapter
                .unregister_session_registration_until_terminal_if_current(&registration)
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "retiring the conflicting runtime registration of session {session_id}: {error}"
                    ))
                })?;
        }
    }
    match session_service.discard_live_session(session_id).await {
        Ok(()) | Err(meerkat_core::service::SessionError::NotFound { .. }) => Ok(()),
        Err(error) => Err(MobError::SessionError(error)),
    }
}

/// The restore failure of a member whose live actor a binding conflict
/// refused: the conflict, and, when its live actor could not be retired,
/// that failure too.
pub(super) async fn refuse_conflicting_live_actor(
    session_service: &dyn MobSessionService,
    session_id: &SessionId,
    conflict: &MobError,
) -> String {
    match retire_conflicting_live_actor(session_service, session_id).await {
        Ok(()) => conflict.to_string(),
        Err(error) => {
            tracing::error!(
                session_id = %session_id,
                %error,
                "a live actor on a conflicting auth binding could not be retired"
            );
            format!("{conflict}; its live actor could not be retired: {error}")
        }
    }
}
