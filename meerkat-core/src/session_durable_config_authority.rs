//! Session durable-config authority — shell adapter over the canonical
//! [`SessionDocumentMachine`](crate::generated::session_document).
//!
//! Every SEMANTIC admission decision for durable session config lives in the
//! canonical machine's durable-config region (folded from the retired
//! `SessionDurableConfigAuthorityMachine` under LUC-524, P0 Dogma Invariant 1):
//!
//! - metadata-persist admission (`schema_version > 0 && model_present`),
//! - build-state-persist consistency admission (mob-tool authority context
//!   must be absent or generated-authority),
//! - build-state restore authorization (the recovery half of the same fact).
//!
//! This module performs only the MECHANICAL work the DSL cannot express: it
//! extracts the typed facts the machine's verdict actually reads from the bulky
//! [`SessionMetadata`](crate::SessionMetadata) /
//! [`SessionBuildState`](crate::SessionBuildState) records and feeds those —
//! and only those — to the machine, then mirrors the machine's admit/reject
//! verdict. It NEVER decides admission itself, and it passes the original typed
//! value through unchanged on admit — no fact is pre-reduced before the machine
//! sees it. The shell does not mirror the full record into machine inputs: a
//! field the verdict never branches on is not an authority input.

use crate::generated::session_document::{self, SessionDocumentEffect, SessionDocumentError};
use crate::{SessionBuildState, SessionMetadata};

/// Error surfaced when the canonical machine rejects a durable-config request.
///
/// Carries the rejection message produced by the canonical
/// [`SessionDocumentMachine`](crate::generated::session_document) so callers
/// keep a stable durable-config error type while the underlying authority is
/// the canonical session-document machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDurableConfigAuthorityError {
    message: String,
}

impl std::fmt::Display for SessionDurableConfigAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionDurableConfigAuthorityError {}

impl From<SessionDocumentError> for SessionDurableConfigAuthorityError {
    fn from(inner: SessionDocumentError) -> Self {
        Self {
            message: inner.to_string(),
        }
    }
}

/// Metadata whose persist the canonical machine has authorized.
#[derive(Debug, Clone)]
pub struct AuthorizedSessionMetadata {
    metadata: SessionMetadata,
}

impl AuthorizedSessionMetadata {
    #[must_use]
    pub fn into_metadata(self) -> SessionMetadata {
        self.metadata
    }
}

/// Build state whose persist the canonical machine has authorized.
#[derive(Debug, Clone)]
pub struct AuthorizedSessionBuildState {
    state: SessionBuildState,
}

impl AuthorizedSessionBuildState {
    #[must_use]
    pub fn into_state(self) -> SessionBuildState {
        self.state
    }
}

fn document_authority() -> session_document::SessionDocumentMachineAuthority {
    session_document::SessionDocumentMachineAuthority::new()
}

/// Drive the metadata-persist admission transition. Mirrors the machine verdict.
fn drive_metadata_persist(
    metadata: &SessionMetadata,
) -> Result<(), SessionDurableConfigAuthorityError> {
    let mut authority = document_authority();
    let effects = authority.authorize_session_metadata_persist(
        u64::from(metadata.schema_version),
        !metadata.model.trim().is_empty(),
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionMetadataPersistAuthorized
        )
    })
}

/// Drive the build-state-persist admission transition.
fn drive_build_state_persist(
    state: &SessionBuildState,
) -> Result<(), SessionDurableConfigAuthorityError> {
    let mob_tool_authority_context_present = state.mob_tool_authority_context.is_some();
    let mob_tool_authority_context_generated = state
        .mob_tool_authority_context
        .as_ref()
        .is_some_and(|context| context.is_generated_authority_context());
    let mut authority = document_authority();
    let effects = authority.authorize_session_build_state_persist(
        mob_tool_authority_context_present,
        mob_tool_authority_context_generated,
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionBuildStatePersistAuthorized
        )
    })
}

/// Drive the build-state restore authorization transition. The machine's
/// restore guard is `Ready`-only — it reads no build-state facts — so this
/// drives the transition without threading the (shell-retained) snapshot.
fn drive_build_state_restore() -> Result<(), SessionDurableConfigAuthorityError> {
    let mut authority = document_authority();
    let effects = authority.restore_session_build_state()?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionBuildStateRestoreAuthorized
        )
    })
}

/// Confirm the machine emitted the expected authorization effect.
///
/// A rejected request matched no transition and already surfaced as `Err`
/// above; this guards against an emitter that drove a different effect.
fn expect_effect(
    effects: &[SessionDocumentEffect],
    matches_expected: impl Fn(&SessionDocumentEffect) -> bool,
) -> Result<(), SessionDurableConfigAuthorityError> {
    if !effects.is_empty() && effects.iter().all(matches_expected) {
        Ok(())
    } else {
        Err(SessionDurableConfigAuthorityError {
            message:
                "generated session document authority emitted no durable-config authorization effect"
                    .to_string(),
        })
    }
}

/// Authorize a session-metadata persist through the canonical machine,
/// stamping the current schema version first.
pub fn authorize_session_metadata_persist(
    mut metadata: SessionMetadata,
) -> Result<AuthorizedSessionMetadata, SessionDurableConfigAuthorityError> {
    metadata.schema_version = crate::session_metadata_schema_version();
    drive_metadata_persist(&metadata)?;
    Ok(AuthorizedSessionMetadata { metadata })
}

/// Authorize restore of persisted session metadata through the canonical
/// machine (same admission contract as persist).
pub fn restore_session_metadata(
    metadata: SessionMetadata,
) -> Result<SessionMetadata, SessionDurableConfigAuthorityError> {
    drive_metadata_persist(&metadata)?;
    Ok(metadata)
}

/// Authorize a session-build-state persist through the canonical machine.
pub fn authorize_session_build_state_persist(
    state: SessionBuildState,
) -> Result<AuthorizedSessionBuildState, SessionDurableConfigAuthorityError> {
    drive_build_state_persist(&state)?;
    Ok(AuthorizedSessionBuildState { state })
}

/// Authorize restore of persisted session build state through the canonical
/// machine.
pub fn restore_session_build_state(
    state: SessionBuildState,
) -> Result<SessionBuildState, SessionDurableConfigAuthorityError> {
    drive_build_state_restore()?;
    Ok(state)
}
