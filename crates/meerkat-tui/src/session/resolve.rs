//! Resolve user-facing session references (`last`, `~N`, full id, id prefix)
//! against the realm's session store, mirroring `rkat`'s
//! `resolve_flexible_session_id`.

use std::cmp::Reverse;

use meerkat::{SessionFilter, SessionStore};
use meerkat_core::{SessionId, SessionMeta};

use crate::error::TuiError;

/// Resolve `reference` to a session id.
///
/// - `last`, `~`, `~0`: the most recently updated session
/// - `~N`: the N-th most recently updated session (0-based)
/// - a full UUID: that id (existence is checked when the session is loaded)
/// - anything else: a unique id prefix
///
/// # Errors
///
/// [`TuiError::SessionRef`] when nothing (or more than one session) matches,
/// [`TuiError::Store`] when listing fails.
pub async fn resolve_session_ref(
    store: &dyn SessionStore,
    reference: &str,
) -> Result<SessionId, TuiError> {
    let reference = reference.trim();
    let fail = |reason: String| TuiError::SessionRef {
        reference: reference.to_string(),
        reason,
    };
    if reference.is_empty() {
        return Err(fail("empty session reference".to_string()));
    }

    let offset =
        match reference {
            "last" | "~" | "~0" => Some(0),
            s => match s.strip_prefix('~') {
                Some(n) => Some(n.parse::<usize>().map_err(|_| {
                    fail("expected ~N where N is a non-negative number".to_string())
                })?),
                None => None,
            },
        };

    if let Some(offset) = offset {
        let sessions = sorted_sessions(store).await?;
        return match sessions.get(offset) {
            Some(meta) => Ok(meta.id.clone()),
            None if sessions.is_empty() => Err(fail("no sessions in this realm".to_string())),
            None => Err(fail(format!(
                "only {} session(s) exist; ~{offset} is out of range",
                sessions.len()
            ))),
        };
    }

    if let Ok(id) = SessionId::parse(reference) {
        return Ok(id);
    }

    let prefix = reference.to_ascii_lowercase();
    let sessions = sorted_sessions(store).await?;
    let mut matches = sessions
        .iter()
        .filter(|meta| meta.id.to_string().starts_with(&prefix));
    match (matches.next(), matches.next()) {
        (Some(only), None) => Ok(only.id.clone()),
        (None, _) => Err(fail("no session matches this id or prefix".to_string())),
        (Some(_), Some(_)) => Err(fail(format!(
            "prefix is ambiguous ({} sessions match); use more characters",
            2 + matches.count()
        ))),
    }
}

/// All sessions, most recently updated first (ties: newest id first). Store
/// backends do not agree on list order, so sort here.
///
/// # Errors
///
/// Returns [`TuiError::Store`] when the store cannot be listed.
pub async fn list_sessions(store: &dyn SessionStore) -> Result<Vec<SessionMeta>, TuiError> {
    sorted_sessions(store).await
}

async fn sorted_sessions(store: &dyn SessionStore) -> Result<Vec<SessionMeta>, TuiError> {
    let mut sessions = store.list(SessionFilter::default()).await?;
    sessions.sort_by_key(|meta| Reverse((meta.updated_at, meta.id.0)));
    Ok(sessions)
}
