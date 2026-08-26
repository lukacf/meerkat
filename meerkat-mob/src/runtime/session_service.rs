use super::*;
use meerkat_core::AgentExecutionSnapshot;
use meerkat_core::CommsCapabilityError;
use meerkat_core::ExternalToolSurfaceSnapshot;
use meerkat_core::PeerIngressRuntimeSnapshot;
use meerkat_core::Session;
use meerkat_core::ToolScopeSnapshot;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, TurnRequestContext};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::service::StartTurnRequest;
use meerkat_core::service::{
    SessionError, SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt,
};
use meerkat_core::{InputId, RunId};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::{Mutex, OnceLock, Weak};

#[cfg(feature = "experimental-gpt-live")]
fn start_live_bridge_on_session_actor(
    accepted: Result<meerkat_session::LiveBridgeSessionOperationTerminalReceiver, SessionError>,
    max_output_bytes: usize,
) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError> {
    let terminal = accepted.map_err(|error| match error {
        SessionError::NotFound { .. } => super::LiveBridgeOperationStartError::Unavailable,
        SessionError::Busy { .. } => super::LiveBridgeOperationStartError::TemporarilyUnavailable,
        SessionError::Agent(meerkat_core::AgentError::ConfigError(_)) => {
            super::LiveBridgeOperationStartError::Rejected
        }
        _ => super::LiveBridgeOperationStartError::Failed,
    })?;
    Ok(Box::pin(async move {
        match terminal.await {
            Ok(Ok(result)) => {
                super::LiveBridgeOperationTerminal::completed(result.text, max_output_bytes)
                    .unwrap_or_else(|_| super::LiveBridgeOperationTerminal::failed())
            }
            Ok(Err(meerkat_core::AgentError::Cancelled)) => {
                super::LiveBridgeOperationTerminal::cancelled()
            }
            Ok(Err(_)) | Err(_) => super::LiveBridgeOperationTerminal::failed(),
        }
    }))
}

/// Outcome of the explicit resume-seam durable read.
///
/// Exists because `Option<Session>` conflated three materially different
/// answers — present, archived-and-refused, and genuinely absent — and every
/// caller reported the union as "missing durable session snapshot". That
/// message sent operators hunting for data that was intact on disk.
#[derive(Debug)]
#[non_exhaustive]
pub enum ResumeSessionLoad {
    /// An ordinary, non-archived durable session.
    Active(Box<Session>),
    /// An archived document the lifecycle machine is able to revive.
    Revivable(Box<Session>),
    /// An archived document that is NOT revivable from its current runtime
    /// state. The transcript is intact; only the lifecycle pairing refuses.
    ArchivedNotRevivable {
        runtime_state: Option<meerkat_runtime::RuntimeState>,
    },
    /// No durable session exists for this id.
    Absent,
}

/// Owner-issued lifecycle fact carried by the operational resume pipeline.
///
/// `NoCurrentDurableAuthority` is intentionally weaker than `NeverPersisted`: the current
/// session-store API proves only that no authoritative body is present at this
/// read. Claiming historical non-existence requires a separate durable
/// provenance fact that this boundary does not expose.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionResumeLifecycle {
    NoCurrentDurableAuthority,
    ContradictoryDurableAuthority {
        runtime_state: Option<meerkat_runtime::RuntimeState>,
        occurrence_generation: Option<u64>,
        head_revision: Option<u64>,
    },
    Archived {
        revivable: bool,
        runtime_state: Option<meerkat_runtime::RuntimeState>,
        head_revision: Option<u64>,
    },
    Active {
        /// `None` means the exact runtime lifecycle observation contains no
        /// bound generation; it is never normalized to generation 0.
        occurrence_generation: Option<u64>,
        head_revision: Option<u64>,
    },
}

/// Actor materialization selected by the owner-issued resume verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionResumeMaterialization {
    Active,
    Revivable,
}

/// Typed reason an operational resume could not produce a session body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeRejectionKind {
    Absent,
    ArchivedNotRevivable,
    CommittedBoundaryUnprovable,
    AuthorityChangedDuringMaterialization,
    ContradictoryDurableAuthority,
}

/// Whether repeating the same durable observation can change the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeVerdictTerminality {
    StableParkable,
    TransientRetryable,
}

/// Typed rejection issued by the session owner rather than reconstructed by
/// downstream callers from store probes or display text.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResumeRejection {
    pub session_id: SessionId,
    pub lifecycle: SessionResumeLifecycle,
    pub authority: Box<SessionResumeAuthority>,
    pub kind: ResumeRejectionKind,
    pub detail: String,
    pub runtime_state: Option<meerkat_runtime::RuntimeState>,
    pub terminality: ResumeVerdictTerminality,
}

impl SessionResumeRejection {
    #[must_use]
    pub fn into_mob_error(self) -> MobError {
        let reason = match self.kind {
            ResumeRejectionKind::Absent => crate::error::SessionResumeUnavailableReason::Absent,
            ResumeRejectionKind::ArchivedNotRevivable => {
                crate::error::SessionResumeUnavailableReason::ArchivedNotRevivable
            }
            ResumeRejectionKind::CommittedBoundaryUnprovable => {
                crate::error::SessionResumeUnavailableReason::CommittedBoundaryUnprovable
            }
            ResumeRejectionKind::AuthorityChangedDuringMaterialization => {
                crate::error::SessionResumeUnavailableReason::AuthorityChangedDuringMaterialization
            }
            ResumeRejectionKind::ContradictoryDurableAuthority => {
                crate::error::SessionResumeUnavailableReason::ContradictoryDurableAuthority
            }
        };
        MobError::SessionUnavailableForResume {
            session_id: self.session_id.clone(),
            reason,
            runtime_state: self.runtime_state.map(|state| state.to_string()),
            verdict: Some(Box::new(self)),
        }
    }
}

/// One backend-atomic store-issued observation captured for a resume verdict.
/// Session authority, catalog lifecycle, and the raw machine-lifecycle row
/// come from the same backend snapshot. Session body materialization remains
/// separate and is accepted only when equal observations bracket the load.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionResumeAuthority {
    pub observation: Option<meerkat_runtime::store::RuntimeSessionResumeObservation>,
}

impl SessionResumeAuthority {
    #[must_use]
    pub fn session_store_authority(
        &self,
    ) -> Option<crate::identity::IdentitySessionStoreAuthority> {
        self.observation
            .as_ref()?
            .session_authority()
            .cloned()
            .map(crate::identity::IdentitySessionStoreAuthority::from_runtime_authority)
    }

    #[must_use]
    pub fn runtime_state(&self) -> Option<meerkat_runtime::RuntimeState> {
        match self.observation.as_ref()?.lifecycle() {
            meerkat_runtime::store::MachineLifecycleObservation::Decoded { record, .. } => {
                record.runtime_state()
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn occurrence_generation(&self) -> Option<u64> {
        self.observation.as_ref()?.runtime_generation()
    }

    #[must_use]
    pub fn head_revision(&self) -> Option<u64> {
        self.observation.as_ref()?.session_store_revision()
    }

    /// Read-only lifecycle classification from the one backend-atomic resume
    /// observation. This does not materialize a Session body or run recovery.
    #[must_use]
    pub fn lifecycle(&self) -> SessionResumeLifecycle {
        SessionResumeVerdict::lifecycle_from_authority(self)
    }
}

/// Operational resume decision after durable-tail convergence.
///
/// This carrier deliberately contains only facts the current owner can prove:
/// lifecycle classification, the store-issued revision for an active body,
/// materialization eligibility, and a typed rejection. Store and
/// runtime lifecycle authority remain owner-issued exact observations; this
/// layer does not synthesize a sequence from independent scalar fields. The
/// body is separately loaded and accepted only between equal atomic authority
/// observations.
#[derive(Debug)]
#[non_exhaustive]
pub enum SessionResumeVerdict {
    ResumeAuthorized {
        lifecycle: SessionResumeLifecycle,
        authority: SessionResumeAuthority,
        materialization: SessionResumeMaterialization,
        session: Box<Session>,
        preparation: SessionResumePreparationReceipt,
    },
    Rejected(SessionResumeRejection),
}

#[derive(Debug)]
pub struct AuthorizedSessionResume {
    pub lifecycle: SessionResumeLifecycle,
    pub authority: SessionResumeAuthority,
    pub materialization: SessionResumeMaterialization,
    pub session: Box<Session>,
    pub(crate) preparation: SessionResumePreparationReceipt,
}

/// Owner-issued proof that the durable committed-boundary preparation used by
/// one bracketed resume materialization has completed.
///
/// The fields are private so callers cannot manufacture a receipt or pair it
/// with a different authority observation. Persistent actor creation consumes
/// the receipt instead of repeating recovery after the mob already holds
/// Prepared+B and has revalidated the exact authority carried here.
#[derive(Debug)]
pub struct SessionResumePreparationReceipt {
    session_id: SessionId,
    authority: SessionResumeAuthority,
    kind: SessionResumePreparationKind,
}

#[derive(Debug)]
enum SessionResumePreparationKind {
    NonPersistent,
    #[cfg(not(target_arch = "wasm32"))]
    PersistentCommittedBoundary(meerkat_session::CommittedBoundaryResumePreparationReceipt),
}

impl SessionResumePreparationReceipt {
    fn issue(
        session_id: &SessionId,
        authority: &SessionResumeAuthority,
        kind: SessionResumePreparationKind,
    ) -> Self {
        Self {
            session_id: session_id.clone(),
            authority: authority.clone(),
            kind,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn from_persistent(
        session_id: &SessionId,
        authority: &SessionResumeAuthority,
        preparation: meerkat_session::CommittedBoundaryResumePreparationReceipt,
    ) -> Self {
        Self {
            session_id: session_id.clone(),
            authority: authority.clone(),
            kind: SessionResumePreparationKind::PersistentCommittedBoundary(preparation),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn into_persistent_for(
        self,
        session_id: &SessionId,
    ) -> Result<meerkat_session::CommittedBoundaryResumePreparationReceipt, SessionError> {
        if &self.session_id != session_id {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "resume preparation receipt for '{}' cannot materialize session '{session_id}'",
                    self.session_id
                )),
            ));
        }
        match self.kind {
            SessionResumePreparationKind::PersistentCommittedBoundary(preparation) => {
                Ok(preparation)
            }
            SessionResumePreparationKind::NonPersistent => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "persistent resume for session '{session_id}' did not carry owner-issued committed-boundary preparation"
                )),
            )),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn advance_after_machine_prepare(
        self,
        prepared: &meerkat_runtime::PreparedSessionMaterialization,
    ) -> Result<Self, SessionError> {
        let Self {
            session_id,
            authority,
            kind,
        } = self;
        let kind = match kind {
            SessionResumePreparationKind::NonPersistent => {
                SessionResumePreparationKind::NonPersistent
            }
            SessionResumePreparationKind::PersistentCommittedBoundary(preparation) => {
                SessionResumePreparationKind::PersistentCommittedBoundary(
                    preparation.advance_after_machine_prepare(prepared).await?,
                )
            }
        };
        Ok(Self {
            session_id,
            authority,
            kind,
        })
    }

    #[cfg(target_arch = "wasm32")]
    async fn advance_after_machine_prepare(
        self,
        _prepared: &meerkat_runtime::PreparedSessionMaterialization,
    ) -> Result<Self, SessionError> {
        Ok(self)
    }

    pub(crate) fn matches_authority(&self, authority: &SessionResumeAuthority) -> bool {
        &self.authority == authority
    }
}

impl SessionResumeVerdict {
    pub fn into_authorized(self) -> Result<AuthorizedSessionResume, SessionResumeRejection> {
        match self {
            Self::ResumeAuthorized {
                lifecycle,
                authority,
                materialization,
                session,
                preparation,
            } => Ok(AuthorizedSessionResume {
                lifecycle,
                authority,
                materialization,
                session,
                preparation,
            }),
            Self::Rejected(rejection) => Err(rejection),
        }
    }

    fn from_authoritative_load_with_authority(
        session_id: &SessionId,
        load: ResumeSessionLoad,
        authority: SessionResumeAuthority,
        preparation: Option<SessionResumePreparationReceipt>,
    ) -> Result<Self, SessionError> {
        let runtime_state = authority.runtime_state();
        let occurrence_generation = authority.occurrence_generation();
        let head_revision = authority.head_revision();
        let authority_lifecycle = Self::lifecycle_from_authority(&authority);
        if matches!(
            &load,
            ResumeSessionLoad::Active(_) | ResumeSessionLoad::Revivable(_)
        ) && authority
            .observation
            .as_ref()
            .is_some_and(|observation| observation.session_authority().is_none())
        {
            return Ok(Self::Rejected(SessionResumeRejection {
                session_id: session_id.clone(),
                lifecycle: Self::lifecycle_from_authority(&authority),
                runtime_state,
                authority: Box::new(authority),
                kind: ResumeRejectionKind::ContradictoryDurableAuthority,
                detail: format!(
                    "session '{session_id}' materialized a body without store-issued session authority"
                ),
                terminality: ResumeVerdictTerminality::StableParkable,
            }));
        }
        match load {
            ResumeSessionLoad::Active(session) => {
                let preparation = preparation.ok_or_else(|| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "active resume for session '{session_id}' omitted owner-issued preparation"
                    )))
                })?;
                let lifecycle = match authority_lifecycle {
                    lifecycle @ SessionResumeLifecycle::Active { .. } => lifecycle,
                    SessionResumeLifecycle::NoCurrentDurableAuthority
                        if authority.observation.is_none() =>
                    {
                        // Ephemeral services have no durable backend
                        // observation. Their body is the only available
                        // operational input, but it does not claim durable
                        // store authority.
                        SessionResumeLifecycle::Active {
                            occurrence_generation,
                            head_revision,
                        }
                    }
                    lifecycle => {
                        return Ok(Self::contradictory_durable_authority(
                            session_id,
                            authority,
                            lifecycle,
                            "active resume load disagrees with atomic durable lifecycle",
                        ));
                    }
                };
                Ok(Self::ResumeAuthorized {
                    lifecycle,
                    authority,
                    materialization: SessionResumeMaterialization::Active,
                    session,
                    preparation,
                })
            }
            ResumeSessionLoad::Revivable(session) => {
                let preparation = preparation.ok_or_else(|| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "revivable resume for session '{session_id}' omitted owner-issued preparation"
                    )))
                })?;
                // The atomic owner observation decides whether this is an
                // archived document or a non-archived document paired with a
                // Retired runtime. The portable Session projection is not
                // lifecycle authority.
                let lifecycle = match authority_lifecycle {
                    lifecycle @ SessionResumeLifecycle::Archived { .. } => lifecycle,
                    lifecycle @ SessionResumeLifecycle::Active { .. }
                        if runtime_state == Some(meerkat_runtime::RuntimeState::Retired) =>
                    {
                        lifecycle
                    }
                    lifecycle => {
                        return Ok(Self::contradictory_durable_authority(
                            session_id,
                            authority,
                            lifecycle,
                            "revivable resume load lacks matching atomic durable lifecycle",
                        ));
                    }
                };
                Ok(Self::ResumeAuthorized {
                    lifecycle,
                    authority,
                    materialization: SessionResumeMaterialization::Revivable,
                    session,
                    preparation,
                })
            }
            ResumeSessionLoad::ArchivedNotRevivable {
                runtime_state: load_runtime_state,
            } => {
                let lifecycle = match authority_lifecycle {
                    lifecycle @ SessionResumeLifecycle::Archived {
                        revivable: false, ..
                    } => lifecycle,
                    lifecycle => {
                        return Ok(Self::contradictory_durable_authority(
                            session_id,
                            authority,
                            lifecycle,
                            "archived refusal disagrees with atomic durable lifecycle",
                        ));
                    }
                };
                if load_runtime_state != runtime_state {
                    return Ok(Self::contradictory_durable_authority(
                        session_id,
                        authority,
                        lifecycle,
                        "archived refusal runtime state disagrees with atomic durable authority",
                    ));
                }
                let state = runtime_state.map_or_else(
                    || "<no runtime record>".to_string(),
                    |state| state.to_string(),
                );
                Ok(Self::Rejected(SessionResumeRejection {
                    session_id: session_id.clone(),
                    lifecycle,
                    authority: Box::new(authority),
                    kind: ResumeRejectionKind::ArchivedNotRevivable,
                    detail: format!(
                        "durable session '{session_id}' is archived and not revivable from runtime state {state}; the transcript is intact and preserved"
                    ),
                    runtime_state,
                    terminality: if runtime_state == Some(meerkat_runtime::RuntimeState::Destroyed)
                    {
                        ResumeVerdictTerminality::StableParkable
                    } else {
                        ResumeVerdictTerminality::TransientRetryable
                    },
                }))
            }
            ResumeSessionLoad::Absent => {
                let positive_authority =
                    authority.observation.as_ref().is_some_and(|observation| {
                        observation.session_authority().is_some()
                            || observation.catalog_entry().is_some()
                            || !matches!(
                                observation.lifecycle(),
                                meerkat_runtime::store::MachineLifecycleObservation::Missing
                            )
                    });
                if positive_authority {
                    Ok(Self::Rejected(SessionResumeRejection {
                        session_id: session_id.clone(),
                        lifecycle: Self::lifecycle_from_authority(&authority),
                        runtime_state: authority.runtime_state(),
                        authority: Box::new(authority),
                        kind: ResumeRejectionKind::ContradictoryDurableAuthority,
                        detail: format!(
                            "session '{session_id}' has durable authority or lifecycle rows but no materializable body"
                        ),
                        terminality: ResumeVerdictTerminality::StableParkable,
                    }))
                } else {
                    Ok(Self::Rejected(SessionResumeRejection {
                        session_id: session_id.clone(),
                        lifecycle: SessionResumeLifecycle::NoCurrentDurableAuthority,
                        authority: Box::new(authority),
                        kind: ResumeRejectionKind::Absent,
                        detail: format!("missing durable session snapshot for '{session_id}'"),
                        runtime_state: None,
                        terminality: ResumeVerdictTerminality::StableParkable,
                    }))
                }
            }
        }
    }

    fn contradictory_durable_authority(
        session_id: &SessionId,
        authority: SessionResumeAuthority,
        lifecycle: SessionResumeLifecycle,
        detail: &str,
    ) -> Self {
        let runtime_state = authority.runtime_state();
        Self::Rejected(SessionResumeRejection {
            session_id: session_id.clone(),
            lifecycle,
            authority: Box::new(authority),
            kind: ResumeRejectionKind::ContradictoryDurableAuthority,
            detail: format!("session '{session_id}' {detail}"),
            runtime_state,
            terminality: ResumeVerdictTerminality::StableParkable,
        })
    }

    fn lifecycle_from_authority(authority: &SessionResumeAuthority) -> SessionResumeLifecycle {
        let runtime_state = authority.runtime_state();
        let head_revision = authority.head_revision();
        match authority.observation.as_ref() {
            None => SessionResumeLifecycle::NoCurrentDurableAuthority,
            Some(observation)
                if observation
                    .catalog_entry()
                    .and_then(|entry| entry.lifecycle_terminal())
                    .is_some_and(meerkat_core::SessionLifecycleTerminal::is_archived) =>
            {
                SessionResumeLifecycle::Archived {
                    revivable: matches!(
                        runtime_state,
                        None | Some(
                            meerkat_runtime::RuntimeState::Idle
                                | meerkat_runtime::RuntimeState::Retired
                        )
                    ),
                    runtime_state,
                    head_revision,
                }
            }
            Some(observation) if observation.session_authority().is_some() => {
                SessionResumeLifecycle::Active {
                    occurrence_generation: authority.occurrence_generation(),
                    head_revision,
                }
            }
            Some(observation)
                if observation.catalog_entry().is_some()
                    || !matches!(
                        observation.lifecycle(),
                        meerkat_runtime::store::MachineLifecycleObservation::Missing
                    ) =>
            {
                SessionResumeLifecycle::ContradictoryDurableAuthority {
                    runtime_state,
                    occurrence_generation: authority.occurrence_generation(),
                    head_revision,
                }
            }
            Some(_) => SessionResumeLifecycle::NoCurrentDurableAuthority,
        }
    }

    fn committed_boundary_unprovable(
        session_id: &SessionId,
        _load: ResumeSessionLoad,
        authority: SessionResumeAuthority,
        detail: String,
    ) -> Result<Self, SessionError> {
        let lifecycle = Self::lifecycle_from_authority(&authority);
        let runtime_state = authority.runtime_state();
        Ok(Self::Rejected(SessionResumeRejection {
            session_id: session_id.clone(),
            lifecycle,
            authority: Box::new(authority),
            kind: ResumeRejectionKind::CommittedBoundaryUnprovable,
            detail,
            runtime_state,
            terminality: ResumeVerdictTerminality::StableParkable,
        }))
    }

    pub(crate) fn authority_changed_during_materialization(
        session_id: &SessionId,
        authority: SessionResumeAuthority,
    ) -> Self {
        let runtime_state = authority.runtime_state();
        let lifecycle = Self::lifecycle_from_authority(&authority);
        Self::Rejected(SessionResumeRejection {
            session_id: session_id.clone(),
            lifecycle,
            authority: Box::new(authority),
            kind: ResumeRejectionKind::AuthorityChangedDuringMaterialization,
            detail: format!(
                "durable authority for session '{session_id}' changed while its resume body was materialized"
            ),
            runtime_state,
            terminality: ResumeVerdictTerminality::TransientRetryable,
        })
    }
}

/// Explicit opt-in for services whose session body has no durable committed
/// boundary to prepare before actor materialization.
///
/// Persistent implementations and decorators must forward or implement the
/// combined owner-issued verdict instead. Refusing services that claim
/// persistence keeps this helper from becoming a silent fallback when a
/// public wrapper forgets to forward that authority seam.
pub async fn materialize_nonpersistent_session_resume_verdict<S>(
    session_service: &S,
    session_id: &SessionId,
) -> Result<SessionResumeVerdict, SessionError>
where
    S: MobSessionService + ?Sized,
{
    if session_service.supports_persistent_sessions() {
        return Err(SessionError::Unsupported(format!(
            "persistent session service must issue an owner-prepared resume verdict for session '{session_id}'"
        )));
    }
    materialize_nonpersistent_session_resume_verdict_inner(session_service, session_id).await
}

/// Crate-private composition seam for persistence-model test doubles that
/// deliberately model only the body/authority bracket and never consume a
/// persistent committed-boundary receipt. Public implementations must use the
/// checked helper above or forward their persistent owner's combined verdict.
#[cfg(test)]
pub(crate) async fn materialize_nonpersistent_session_resume_verdict_unchecked<S>(
    session_service: &S,
    session_id: &SessionId,
) -> Result<SessionResumeVerdict, SessionError>
where
    S: MobSessionService + ?Sized,
{
    materialize_nonpersistent_session_resume_verdict_inner(session_service, session_id).await
}

async fn materialize_nonpersistent_session_resume_verdict_inner<S>(
    session_service: &S,
    session_id: &SessionId,
) -> Result<SessionResumeVerdict, SessionError>
where
    S: MobSessionService + ?Sized,
{
    let before = session_service
        .observe_session_resume_authority(session_id)
        .await?;
    let load = session_service.load_session_for_resume(session_id).await?;
    let authority = session_service
        .observe_session_resume_authority(session_id)
        .await?;
    if before != authority {
        return Ok(
            SessionResumeVerdict::authority_changed_during_materialization(session_id, authority),
        );
    }
    let preparation = SessionResumePreparationReceipt::issue(
        session_id,
        &authority,
        SessionResumePreparationKind::NonPersistent,
    );
    SessionResumeVerdict::from_authoritative_load_with_authority(
        session_id,
        load,
        authority,
        Some(preparation),
    )
}

/// Typed actor-materialization route selected by the mob provisioner after
/// the durable resume decision and the runtime machine preparation agree.
///
/// This is deliberately not an `Option<Archived...Authorization>`: absence
/// previously meant both "fresh or ordinary resume" and "revivable resume
/// whose authorization was accidentally dropped". Keeping the route typed
/// makes every cold-boot actor creation pass through one execution seam while
/// preserving the exact authority required by each path.
#[cfg(feature = "runtime-adapter")]
pub(crate) enum SessionActorMaterializationRoute {
    /// Create a newly admitted session actor.
    Fresh,
    /// Recreate an actor from an active durable session body.
    Resume {
        preparation: SessionResumePreparationReceipt,
    },
    /// Recreate an actor from a machine-authorized revivable session body.
    Revivable {
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        preparation: SessionResumePreparationReceipt,
    },
    /// Recreate only the actor for an already-serving exact attachment.
    AttachedActorRecovery {
        preparation: SessionResumePreparationReceipt,
    },
}

#[cfg(feature = "runtime-adapter")]
impl SessionActorMaterializationRoute {
    #[must_use]
    pub(crate) fn is_revivable(&self) -> bool {
        matches!(self, Self::Revivable { .. })
    }

    pub(crate) async fn advance_resume_preparation_after_machine_prepare(
        self,
        prepared: &meerkat_runtime::PreparedSessionMaterialization,
    ) -> Result<Self, SessionError> {
        match self {
            Self::Fresh => Ok(Self::Fresh),
            Self::Resume { preparation } => Ok(Self::Resume {
                preparation: preparation.advance_after_machine_prepare(prepared).await?,
            }),
            Self::Revivable {
                authorization,
                preparation,
            } => Ok(Self::Revivable {
                authorization,
                preparation: preparation.advance_after_machine_prepare(prepared).await?,
            }),
            Self::AttachedActorRecovery { preparation } => Ok(Self::AttachedActorRecovery {
                preparation: preparation.advance_after_machine_prepare(prepared).await?,
            }),
        }
    }
}

/// Worst-case read shape of [`MobSessionService::observe_persisted_session_authority`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedSessionAuthorityReadCost {
    /// The service does not expose exact persisted-boundary authority.
    Unsupported,
    /// One bounded authority-row read, independent of transcript size.
    Bounded,
}

fn build_runtime_receipt(
    run_id: RunId,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    session: &Session,
) -> Result<RunBoundaryReceiptDraft, SessionError> {
    // MUST mint the digest the committed-boundary witness validation
    // recomputes (`Session::transcript_content_digest`, the accumulator's
    // canonical `sha256:<hex>` format, O(delta) on appends). The persistent
    // producers were switched with the format change; this ephemeral
    // runtime-adapter producer was missed, and a bare serde-JSON hash here
    // fails every completed-run commit with a digest mismatch.
    let conversation_digest = session.transcript_content_digest().map_err(|err| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "failed to digest session for runtime receipt: {err}"
        )))
    })?;
    Ok(RunBoundaryReceiptDraft {
        run_id,
        boundary,
        contributing_input_ids,
        conversation_digest: Some(conversation_digest),
        message_count: session.messages().len(),
    })
}

#[cfg(feature = "runtime-adapter")]
fn ephemeral_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
fn persistent_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(feature = "runtime-adapter")]
fn cached_runtime_adapter(
    cache: &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>,
    key: usize,
    init: impl FnOnce() -> Arc<meerkat_runtime::MeerkatMachine>,
) -> Arc<meerkat_runtime::MeerkatMachine> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|_, adapter| adapter.strong_count() > 0);
    if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
        return existing;
    }
    let adapter = init();
    cache.insert(key, Arc::downgrade(&adapter));
    adapter
}

#[cfg(feature = "runtime-adapter")]
pub(crate) async fn retire_runtime_session_for_archive(
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    // No shell phase probes: the machine owns every retire verdict.
    // `Retire` admits Idle/Attached/Running/Stopped (Stopped retires
    // durably instead of the former silent early-return that stranded
    // stopped sessions un-retired), and `RetireAlreadyRetired` no-ops a
    // Retired machine. An unregistered session registers first — recovery
    // seeds the durable phase and the retire then lands as a machine
    // transition on it.
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    match meerkat_runtime::RuntimeControlPlane::retire(runtime_adapter, &runtime_id).await {
        Ok(_) => Ok(()),
        Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
            runtime_adapter
                .register_session(session_id.clone())
                .await
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive register before retire failed: {error}"
                    )))
                })?;
            meerkat_runtime::RuntimeControlPlane::retire(runtime_adapter, &runtime_id)
                .await
                .map(|_| ())
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive retire failed after registration: {error}"
                    )))
                })
        }
        Err(error) => Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "machine archive retire failed: {error}"
            )),
        )),
    }
}

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services used by the mob runtime.
///
/// Builds on `SessionServiceCommsExt` from core so mob orchestration can use
/// comms/injector access without per-crate bridge traits.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MobSessionService:
    SessionServiceCommsExt + SessionServiceControlExt + SessionServiceHistoryExt
{
    /// Commit one provider-final client-delegation transcript through the
    /// canonical session actor and SessionDocument authority.
    ///
    /// The provider observation is not executor authority. Only the sealed
    /// evidence returned by this method may be reconciled by the live runtime
    /// before any delegated model or tool work starts.
    #[cfg(feature = "experimental-gpt-live")]
    async fn commit_live_delegation_final_transcript(
        &self,
        _session_id: &SessionId,
        _provisional: meerkat_core::ProvisionalLiveHandoff,
        _final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        Err(SessionError::Unsupported(
            "session service cannot commit a live delegation final transcript".into(),
        ))
    }

    /// Validate the exact current durable member's bridge policy and isolated
    /// client capability before any live channel/provider open.
    #[cfg(feature = "experimental-gpt-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "session service cannot preflight live bridge member eligibility".into(),
        ))
    }

    /// Capture one exact live actor Session clone before bridge admission.
    ///
    /// The returned opaque snapshot owns the revision later sealed by the
    /// machine. Callers must retain and execute this identical clone; a
    /// post-admission re-read is forbidden because full-duplex input may have
    /// advanced the actor in between.
    #[cfg(feature = "experimental-gpt-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &SessionId,
        agent_identity: &str,
    ) -> Result<super::LiveBridgeExecutionSnapshot, SessionError> {
        let _ = (session_id, agent_identity);
        Err(SessionError::Unsupported(
            "session service cannot capture an exact live bridge execution snapshot".into(),
        ))
    }

    /// Start one accepted noncommitting operation on the already-materialized
    /// durable member's session actor.
    #[cfg(feature = "experimental-gpt-live")]
    async fn start_live_bridge_member_operation(
        &self,
        _request: super::LiveBridgeOperationRequest,
        _cancellation: super::LiveBridgeOperationCancellationSignal,
    ) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError>
    {
        Err(super::LiveBridgeOperationStartError::Unavailable)
    }

    /// Create while the caller already owns this session's stable runtime-turn
    /// finalization boundary. Persistent implementations override this to use
    /// their non-reentrant boundary-aware admission seam; simple/mock services
    /// may use ordinary creation because they have no nested boundary owner.
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError>;

    /// Create while B is held and publish the exact live actor incarnation as
    /// soon as registry insertion succeeds. Every service that exposes a
    /// runtime adapter must implement this with its actor-owning registry;
    /// registry-less services cannot participate in runtime-backed attachment.
    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _resume_preparation: Option<SessionResumePreparationReceipt>,
        _actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service cannot publish exact actor identity during boundary-owned create"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support machine-authorized archived resume".into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support boundary-owned machine-authorized archived resume"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        _resume_preparation: SessionResumePreparationReceipt,
        _actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support exact-actor boundary-owned machine-authorized archived resume"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn authorize_revivable_retired_session(
        &self,
        _session_id: &SessionId,
        _authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support exact retired-session authorization".into(),
        ))
    }
    /// Subscribe to session-wide events regardless of triggering interaction.
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        <Self as SessionService>::subscribe_session_events(self, session_id).await
    }

    /// Whether this service satisfies the persistent-session contract required
    /// by REQ-MOB-030.
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    /// Cost contract for exact persisted-session authority observation.
    fn persisted_session_authority_read_cost(&self) -> PersistedSessionAuthorityReadCost {
        PersistedSessionAuthorityReadCost::Unsupported
    }

    /// Observe the runtime store's exact committed session boundary.
    ///
    /// The default is an explicit refusal: implementations must never silently
    /// derive this authority from a Session body or projection-store row.
    async fn observe_persisted_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::identity::IdentitySessionStoreAuthority>, SessionError> {
        Err(SessionError::Unsupported(format!(
            "session service cannot observe exact persisted authority for {session_id}"
        )))
    }

    /// Mechanical presence of the live session actor, without exporting its
    /// document or reconciling durable authority. Health/revival uses this
    /// process-carrier witness so an actor busy inside a provider turn cannot
    /// block observation or be mistaken for dead state.
    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Self as SessionService>::has_live_session(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        None
    }

    /// Whether this service implements the runtime-owned turn application
    /// boundary used by [`MeerkatMachine`]. Merely exposing a runtime adapter
    /// is not sufficient: some hosts use that adapter only for lifecycle or
    /// comms authority and still execute turns through the direct
    /// [`SessionService::start_turn`] path.
    ///
    /// Per-turn LLM identity overrides are legal only when this capability is
    /// true, because the executor must apply the identity immediately before
    /// the exact admitted turn runs.
    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        false
    }

    /// Apply a live hard cancel after `MeerkatMachine` accepts the cancel command.
    ///
    /// The machine has ALREADY admitted the interrupt when this runs — the
    /// implementation applies it to the LIVE agent (e.g. the ephemeral
    /// session task's interrupt slot) and returns. It must NOT call back
    /// into `MeerkatMachine::hard_cancel_current_run`: this method executes
    /// inside the machine's interrupt dispatch, so re-entering the machine
    /// forms a dispatch ring (deadlock or unbounded recursion).
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "interrupt for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    /// Apply a live hard cancel to one exact run after machine admission.
    /// Implementations must return `false` for a stale run and must never
    /// widen the request to the ambient successor.
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        _expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        Err(SessionError::Unsupported(format!(
            "exact-run interrupt for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    /// Apply a live cooperative boundary cancel after `MeerkatMachine` accepts the command.
    ///
    /// The machine has ALREADY admitted the cancel when this runs — the
    /// implementation applies it to the LIVE agent (e.g. the ephemeral
    /// session task's cancel-after-boundary channel) and returns. It must
    /// NOT call back into `MeerkatMachine::cancel_after_boundary`: this
    /// method executes inside the machine's boundary-cancel dispatch, so
    /// re-entering the machine forms a dispatch ring (the machine's
    /// `boundary_cancel_dispatch_pending` fact bounds it to one extra lap,
    /// but the re-entrant lap is still a contract violation).
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "cancel_after_boundary for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    /// Apply a queued attachment-local cooperative cancel using explicit
    /// current-run semantics. Cloneable boundary handles must use the exact-run
    /// method above.
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "current-run cancel_after_boundary for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    async fn execution_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        Ok(None)
    }

    async fn tool_scope_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        Ok(None)
    }

    async fn external_tool_surface_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        Ok(None)
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        Ok(None)
    }

    /// Whether the mob archive authority owns this session's durable record.
    ///
    /// Disposal routes on this fact: owned sessions archive through the mob
    /// authority (where a mid-archive record loss stays a fail-closed
    /// split-state error), while sessions adopted from a host-owned store
    /// (`MemberLaunchMode::Resume` over a service the mob does not own, e.g.
    /// an embedder's console sessions) retire their runtime and release the
    /// binding without touching the authority — archiving a session the mob
    /// never owned is not this mob's to perform.
    ///
    /// Default `true`: a service without a durable read seam claims every
    /// session it is asked about, preserving the fail-closed archive path.
    /// Persistent services override this with a real store read.
    async fn session_known_to_archive_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(true)
    }

    /// Whether a listed session belongs to the given mob for reconciliation.
    ///
    /// Default: `false`. The wave-a demolition removed the comms-name matching
    /// probe; wave-c was intended to land a runtime-aware replacement but did
    /// not, so this remains a no-op default. Persistent services may override
    /// to implement real reconciliation; the ephemeral default treats no
    /// listed session as "belongs to mob".
    async fn session_belongs_to_mob(
        &self,
        _session_id: &SessionId,
        _mob_id: &crate::ids::MobId,
    ) -> bool {
        false
    }

    /// Load the persisted session snapshot when available.
    ///
    /// Default: `Ok(None)`. Matches the pre-demolition behavior where services
    /// without durable persistence returned no snapshot, letting callers treat
    /// the session as missing and fall through to recreate-from-roster paths.
    async fn load_persisted_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
    }

    /// Persist a transcript fork for a concrete new mob identity.
    ///
    /// Durable services override this with their store-owned fork authority.
    /// The target binding is applied before child commit so the ordinary
    /// resume pipeline can validate, rather than invent, the child identity.
    async fn fork_persisted_session(
        &self,
        _source_session_id: &SessionId,
        _message_count: Option<usize>,
        _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        _target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not expose durable transcript fork authority".into(),
        ))
    }

    /// Load an archived session only for an explicit resume/revival operation.
    /// Ordinary reads remain archive-filtered.
    ///
    /// Eligibility is the canonical durable document terminal FIRST — the
    /// `SessionDocumentMachine` lifecycle terminal owns terminality and the
    /// runtime record is only its realization. The runtime state is then
    /// consulted solely to confirm quiescence, and the admitted set matches
    /// exactly what the downstream revival transaction already accepts
    /// (`revive_archived_session_document_inner`): `Retired`, or a
    /// cold-normalized `Idle`.
    ///
    /// This method previously required `RuntimeState::Retired` and nothing
    /// else. That gate was contract drift: retiring an identity stamps the
    /// archived terminal on the durable document without leaving a `Retired`
    /// runtime record, so an intact `Archived + Idle` document was hidden by
    /// the ordinary loader AND rejected here, then reported as a missing
    /// snapshot. Prefer [`Self::load_session_for_resume`], which distinguishes
    /// "archived but not revivable" from "absent" instead of collapsing both
    /// into `None`.
    async fn load_revivable_retired_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
    }

    /// Typed resume-seam read: never collapses "archived", "absent", and
    /// "archived but not revivable" into one `None`.
    ///
    /// REQUIRED, deliberately without a default: a composed fallback over the
    /// two legacy optional reads can never produce
    /// [`ResumeSessionLoad::ArchivedNotRevivable`], so a persistent
    /// implementation compiling against such a default would silently
    /// misreport archived-but-intact documents as absent. Implementations
    /// whose storage genuinely cannot hold an archived-but-intact document
    /// (in-memory services, test doubles) implement the two-read composition
    /// explicitly, as a statement of that fact.
    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError>;

    /// Capture the exact session authority, catalog entry, and runtime
    /// lifecycle row used by an operational resume verdict in one backend
    /// snapshot. The Session body is deliberately not part of that store
    /// snapshot, so operational materialization brackets its body load with
    /// equal observations. Ephemeral services truthfully return an empty
    /// bundle.
    async fn observe_session_resume_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError>;

    /// Re-observe the store-owned resume authority immediately before an
    /// already-authorized body is handed to actor creation. The caller must
    /// retain its Prepared runtime claim while performing this check, so a
    /// body authorized before preparation cannot be consumed under a later
    /// lifecycle or store revision.
    async fn revalidate_session_resume_authority(
        &self,
        session_id: &SessionId,
        expected: &SessionResumeAuthority,
    ) -> Result<Result<(), SessionResumeRejection>, SessionError> {
        let current = self.observe_session_resume_authority(session_id).await?;
        if &current == expected {
            Ok(Ok(()))
        } else {
            let SessionResumeVerdict::Rejected(rejection) =
                SessionResumeVerdict::authority_changed_during_materialization(session_id, current)
            else {
                unreachable!("authority-change constructor always rejects")
            };
            Ok(Err(rejection))
        }
    }

    /// Operational resume composition: first converge durable-tail authority,
    /// then issue one typed decision over the resulting committed body and
    /// retain the exact observations available from each authority owner.
    /// Downstream heal/resume consumers must use this owner-issued verdict
    /// rather than probing session and runtime stores independently.
    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError>;

    /// Load the persisted session METADATA view when available.
    ///
    /// Metadata-only sibling of [`Self::load_persisted_session`] (mobkit
    /// ask-24 clause 3): ownership routing, policy resolution, and presence
    /// probes that only need session-authority metadata facts must not force
    /// a full transcript materialization. Visibility parity with
    /// `load_persisted_session` is part of the contract: whenever that method
    /// returns `Ok(None)` (absent or archived), this one does too.
    ///
    /// Default: derives from `load_persisted_session`, so every backend
    /// exposes the seam with identical visibility. Persistent services
    /// override this with the metadata-only authoritative read. Corrupt
    /// durable metadata is a read FAULT (`Err`), never `Ok(None)`.
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        let Some(session) = self.load_persisted_session(session_id).await? else {
            return Ok(None);
        };
        meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
            .map(Some)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "session {session_id} durable metadata failed typed restore: {err}"
                )))
            })
    }

    /// Archive a mob-owned session through the strongest lifecycle authority
    /// this service exposes. Runtime-backed persistent services override this
    /// to require a concrete `MeerkatMachine` archive protocol before writing
    /// archive projection state.
    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if self.runtime_adapter().is_some() {
            return Err(SessionError::Unsupported(format!(
                "archive for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
            )));
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    /// Archive while the caller owns the exact session turn-finalization
    /// boundary. Persistent implementations must use a non-reentrant service
    /// seam; the default is suitable only for services whose boundary is a
    /// no-op.
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;

    /// Deadline-aware sibling used by retirement. Services with a
    /// process-owned archive implementation override this to preserve the
    /// caller's one absolute teardown budget; simpler services may retain the
    /// required archive contract above.
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        _deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "session service must implement deadline-aware mob archive for session {session_id}"
        )))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
        post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
    ) -> Result<(), SessionError> {
        if post_commit_hook.is_some() {
            return Err(SessionError::Unsupported(format!(
                "session service cannot run a pre-retire archive hook for session {session_id}"
            )));
        }
        self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
            session_id, deadline,
        )
        .await
    }

    async fn apply_runtime_turn(
        &self,
        _session_id: &SessionId,
        _run_id: RunId,
        _req: StartTurnRequest,
        _boundary: RunApplyBoundary,
        _contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime-backed apply is unavailable for this session service".into(),
            ),
        ))
    }

    /// Prepare one exact already-active LLM boundary. Success means the actor
    /// is parked and owned by the returned non-clone commit/abort authority.
    async fn prepare_transient_turn_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        contexts: Vec<TurnRequestContext>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        let _ = (session_id, expected_run_id, contexts);
        Err(meerkat_core::CoreBoundaryStageError::unavailable(
            "session service does not support exact active-turn boundary preparation",
        ))
    }

    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        _session_id: &SessionId,
        _session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    /// Acquire the stable session mutation boundary shared by runtime turns,
    /// direct turns, and non-turn durable writers. The latter includes archive
    /// projection and abnormal/exit teardown recovery, not only ordinary turn
    /// finalization.
    ///
    /// This guard serializes one admitted mutation interval. It does not close
    /// admission, drain future work, or permanently revoke writers; callers
    /// that need terminal quiescence must await the owning lifecycle operation.
    /// Ephemeral/custom services without such a boundary retain the no-op
    /// default.
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        _session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(()))
    }

    /// Checkpoint when the caller already owns the stable outer boundary.
    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        self.checkpoint_committed_runtime_session_snapshot(session_id, session_snapshot)
            .await
    }

    /// Confirm one exact store-issued session boundary while the outer
    /// turn-finalization boundary is held.
    ///
    /// REQUIRED, deliberately without a default: wrappers must forward the
    /// exhaustive authority carrier as one contract. They cannot compile while
    /// accidentally inheriting an `Unsupported` or no-op implementation for
    /// one persistence profile.
    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError>;

    /// Project the exact durable parent-session boundary after this caller's
    /// runtime input reaches successful machine completion. Implementations
    /// must derive both the sealed document and store-issued authority from
    /// canonical session ownership; the Mob shell supplies neither.
    #[cfg(feature = "experimental-gpt-live")]
    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        _session_id: &SessionId,
        _runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        if self.supports_persistent_sessions() {
            return Err(SessionError::Unsupported(
                "persistent MobSessionService wrappers must delegate committed parent-session boundary projection to canonical session ownership"
                    .to_string(),
            ));
        }
        Ok(0)
    }

    /// Remove the service-side live actor while the owning runtime entry is in
    /// its generated post-stop unregister window.
    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session(session_id).await
    }

    /// Cleanup when the caller already owns the stable outer boundary.
    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session_after_runtime_stop_terminalized(session_id)
            .await
    }

    /// Publish predecessor terminals only through one service-minted actor
    /// incarnation. Runtime retirement drops its mutation gate before calling
    /// arbitrary publication code, so resolving only by SessionId here would
    /// allow a delayed predecessor callback to target a successor actor.
    async fn publish_interaction_terminals_for_actor(
        &self,
        _actor_witness: &meerkat_session::LiveSessionActorWitness,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        Err(SessionError::Unsupported(
            "exact interaction terminal publication requires actor-incarnation authority"
                .to_string(),
        ))
    }

    async fn discard_live_session(&self, _session_id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    /// Remove a live actor while the caller already owns its stable outer
    /// turn-finalization boundary.
    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;

    /// Compare-and-remove one exact actor while the caller already owns B.
    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        Err(SessionError::Unsupported(format!(
            "session service cannot discard exact live actor for {}",
            witness.session_id()
        )))
    }

    /// Discard only process-local material for one exact actor after the
    /// runtime store lost durable write authority. The machine's exact
    /// degraded-registration coordinator owns the caller, so implementations
    /// must not acquire the turn-finalization boundary or persist terminal
    /// session or runtime state.
    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        Err(SessionError::Unsupported(format!(
            "session service cannot run durability-reload actor discard for {}",
            witness.session_id()
        )))
    }

    /// Await terminal drain of the current live incarnation's durable event
    /// projection after its producer has been quiesced and discarded.
    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        let _ = session_id;
        Ok(false)
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call in-flight saves complete but subsequent checkpoint
    /// calls on any session are no-ops. Call during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates cancelled by [`cancel_all_checkpointers`].
    ///
    /// Call during `resume()` to restore periodic persistence.
    async fn rearm_all_checkpointers(&self) {}
}

/// Execute the one typed actor-materialization pipeline while the caller owns
/// the stable runtime-turn boundary.
///
/// The provisioner decides the route only after durable resume preparation
/// and machine preparation. This function is the sole lowering owner from
/// that decision into the service's exact create primitive, so a revivable
/// session cannot silently fall through to ordinary create and actor-only
/// recovery cannot accidentally consume archived-resume authority.
#[cfg(feature = "runtime-adapter")]
pub(crate) async fn execute_session_actor_materialization_under_runtime_turn_boundary(
    session_service: &dyn MobSessionService,
    req: meerkat_core::service::CreateSessionRequest,
    route: SessionActorMaterializationRoute,
    actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
) -> Result<meerkat_core::RunResult, SessionError> {
    match route {
        SessionActorMaterializationRoute::Fresh => {
            session_service
                .create_session_with_actor_witness_under_runtime_turn_boundary(
                    req,
                    None,
                    actor_witness_slot,
                )
                .await
        }
        SessionActorMaterializationRoute::Resume { preparation }
        | SessionActorMaterializationRoute::AttachedActorRecovery { preparation } => {
            session_service
                .create_session_with_actor_witness_under_runtime_turn_boundary(
                    req,
                    Some(preparation),
                    actor_witness_slot,
                )
                .await
        }
        SessionActorMaterializationRoute::Revivable {
            authorization,
            preparation,
        } => {
            session_service
                .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                    req,
                    authorization,
                    preparation,
                    actor_witness_slot,
                )
                .await
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<B> MobSessionService for meerkat_session::EphemeralSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    #[cfg(feature = "experimental-gpt-live")]
    async fn commit_live_delegation_final_transcript(
        &self,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        self.commit_live_user_transcript_final(session_id, provisional, Some(final_event))
            .await
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.validate_live_bridge_member_eligibility(session_id)
            .await
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn start_live_bridge_member_operation(
        &self,
        request: super::LiveBridgeOperationRequest,
        cancellation: super::LiveBridgeOperationCancellationSignal,
    ) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError>
    {
        start_live_bridge_on_session_actor(
            self.start_live_bridge_operation(
                request.admission().session_id(),
                request.session_operation_request()?,
                cancellation.receiver(),
            )
            .await,
            request.max_output_bytes(),
        )
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &SessionId,
        agent_identity: &str,
    ) -> Result<super::LiveBridgeExecutionSnapshot, SessionError> {
        loop {
            let authority = self
                .observe_session_transcript_authority(session_id)
                .await?;
            if let Some(session) = self
                .export_session_if_transcript_authority(session_id, authority)
                .await?
            {
                return super::LiveBridgeExecutionSnapshot::from_generation_bound_session(
                    session,
                    agent_identity,
                )
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        error.to_string(),
                    ))
                });
            }
        }
    }

    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        materialize_nonpersistent_session_resume_verdict(self, session_id).await
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Self as meerkat_core::service::SessionService>::create_session(self, req).await
    }

    async fn observe_session_resume_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        Ok(SessionResumeAuthority::default())
    }

    /// In-memory service: nothing durable survives archive, so the two-read
    /// composition is the exact truth — `ArchivedNotRevivable` cannot exist.
    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        if let Some(session) = self.load_persisted_session(session_id).await? {
            return Ok(ResumeSessionLoad::Active(Box::new(session)));
        }
        if let Some(session) = self.load_revivable_retired_session(session_id).await? {
            return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
        }
        Ok(ResumeSessionLoad::Absent)
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        _resume_preparation: Option<SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        #[cfg(feature = "runtime-adapter")]
        let mut actor_materialization_permit = if let Some(bindings) =
            req.build
                .as_ref()
                .and_then(|build| match &build.runtime_build_mode {
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => Some(bindings),
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                }) {
            match meerkat_runtime::begin_session_runtime_actor_materialization(bindings) {
                Ok(permit) => Some(permit),
                Err(meerkat_runtime::RuntimeActorMaterializationError::RegistrationClosed) => {
                    return Err(SessionError::NotFound {
                        id: bindings.session_id().clone(),
                    });
                }
                Err(meerkat_runtime::RuntimeActorMaterializationError::InvalidAuthority(
                    reason,
                )) => {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(reason),
                    ));
                }
            }
        } else {
            None
        };

        let (result, actor_witness) =
            meerkat_session::EphemeralSessionService::<B>::create_session_with_admission_and_witness(
                self,
                req,
                None,
                Some(actor_witness_slot),
            )
            .await?;

        #[cfg(feature = "runtime-adapter")]
        if let Some(permit) = actor_materialization_permit.take()
            && let Err(error) = permit.commit()
        {
            let cleanup =
                meerkat_session::EphemeralSessionService::<B>::discard_live_session_actor(
                    self,
                    &actor_witness,
                )
                .await;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(match cleanup {
                    Ok(_) => format!(
                        "runtime actor materialization commit failed for session {}: {error}",
                        result.session_id
                    ),
                    Err(cleanup_error) => format!(
                        "runtime actor materialization commit failed for session {}: {error}; exact actor cleanup also failed: {cleanup_error}",
                        result.session_id
                    ),
                }),
            ));
        }

        #[cfg(not(feature = "runtime-adapter"))]
        let _ = actor_witness;
        Ok(result)
    }

    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(
            meerkat_session::EphemeralSessionService::<B>::live_session_actor_registered(
                self, session_id,
            )
            .await,
        )
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        let key = std::ptr::from_ref(self) as usize;
        Some(cached_runtime_adapter(
            ephemeral_runtime_adapter_cache(),
            key,
            || Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        true
    }

    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            meerkat_session::EphemeralSessionService::<B>::acquire_runtime_turn_finalization_guard(
                self, session_id,
            )
            .await,
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::interrupt(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::interrupt_run_if_current(
            self,
            session_id,
            expected_run_id,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::cancel_after_boundary_for_run(
            self,
            session_id,
            expected_run_id,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::cancel_after_boundary(self, session_id).await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Self as SessionService>::read(self, session_id).await?;
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            retire_runtime_session_for_archive(runtime_adapter.as_ref(), session_id).await?;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.archive_with_mob_lifecycle_authority(session_id).await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        _deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        // MemberSessionDisposalArc transfers the held B lease and this entire
        // first-party ephemeral archive into a process-owned task before it
        // waits on the owner deadline. This explicit implementation records
        // that contract instead of inheriting a deadline-ignoring default.
        self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::execution_snapshot(self, session_id).await
    }

    /// Export the LIVE session: for the ephemeral backend the live session is
    /// the canonical session, and callers that read session-owned facts
    /// through this seam (e.g. parent tool-access-policy resolution for
    /// spawn/fork `Inherit`) must see it. The trait default's `Ok(None)`
    /// would silently coalesce "session invisible to this read seam" into
    /// "session has no such fact" — on the policy path that is a fail-open
    /// containment escape (a restricted parent minting unrestricted
    /// children).
    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        match meerkat_session::EphemeralSessionService::<B>::export_session(self, session_id).await
        {
            Ok(session) => Ok(Some(session)),
            // A genuinely absent session is the documented `None` shape;
            // every other fault propagates so policy resolution fails closed.
            Err(SessionError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
            Err(error) => Err(SessionError::Unsupported(error.to_string())),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::EphemeralSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session_actor(self, witness)
            .await
    }

    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        // Ephemeral actors are process-local by construction and have no
        // checkpointer sidecars; the exact compare-and-remove is the whole
        // degraded cleanup and persists nothing.
        meerkat_session::EphemeralSessionService::<B>::discard_live_session_actor(self, witness)
            .await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let run_result = meerkat_session::EphemeralSessionService::<B>::start_turn_under_runtime_turn_finalization_boundary(
            self,
            session_id,
            req,
        )
        .await?;
        let session =
            meerkat_session::EphemeralSessionService::<B>::export_session(self, session_id).await?;
        let receipt = build_runtime_receipt(run_id, boundary, contributing_input_ids, &session)?;
        CoreApplyOutput::with_run_result(receipt, None, run_result)
            .with_session(std::sync::Arc::new(session))
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to seal typed session snapshot for runtime commit: {err}"
                )))
            })
    }

    async fn prepare_transient_turn_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        contexts: Vec<TurnRequestContext>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        let prepared =
            meerkat_session::EphemeralSessionService::<B>::prepare_transient_turn_context_for_active_turn(
                self,
                session_id,
                expected_run_id,
                contexts,
            )
            .await?;
        Ok(prepared.into_stage_output(None))
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        _session_id: &SessionId,
        _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "ephemeral session service cannot acknowledge store-owned runtime boundaries"
                .to_string(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    #[cfg(feature = "experimental-gpt-live")]
    async fn commit_live_delegation_final_transcript(
        &self,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        self.commit_live_user_transcript_final(session_id, provisional, Some(final_event))
            .await
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.validate_live_bridge_member_eligibility(session_id)
            .await
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn start_live_bridge_member_operation(
        &self,
        request: super::LiveBridgeOperationRequest,
        cancellation: super::LiveBridgeOperationCancellationSignal,
    ) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError>
    {
        start_live_bridge_on_session_actor(
            self.start_live_bridge_operation(
                request.admission().session_id(),
                request.session_operation_request()?,
                cancellation.receiver(),
            )
            .await,
            request.max_output_bytes(),
        )
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &SessionId,
        agent_identity: &str,
    ) -> Result<super::LiveBridgeExecutionSnapshot, SessionError> {
        let session = self.export_live_session(session_id).await?;
        super::LiveBridgeExecutionSnapshot::from_generation_bound_session(session, agent_identity)
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    error.to_string(),
                ))
            })
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_admission_under_runtime_turn_boundary(req, admission)
            .await
    }

    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        match self.prepare_committed_boundary_resume(session_id).await? {
            meerkat_session::PreparedCommittedBoundaryResume::Materializable {
                session,
                materialization,
                observation,
                preparation,
            } => {
                let authority = SessionResumeAuthority {
                    observation: Some(observation),
                };
                let load = match materialization {
                    meerkat_session::PreparedCommittedBoundaryResumeMaterialization::Active => {
                        ResumeSessionLoad::Active(session)
                    }
                    meerkat_session::PreparedCommittedBoundaryResumeMaterialization::Revivable => {
                        ResumeSessionLoad::Revivable(session)
                    }
                };
                let preparation = SessionResumePreparationReceipt::from_persistent(
                    session_id,
                    &authority,
                    preparation,
                );
                SessionResumeVerdict::from_authoritative_load_with_authority(
                    session_id,
                    load,
                    authority,
                    Some(preparation),
                )
            }
            meerkat_session::PreparedCommittedBoundaryResume::Unavailable {
                unavailable,
                observation,
            } => {
                let authority = SessionResumeAuthority {
                    observation: Some(observation),
                };
                let load = match unavailable {
                    meerkat_session::PreparedCommittedBoundaryResumeUnavailable::Absent => {
                        ResumeSessionLoad::Absent
                    }
                    meerkat_session::PreparedCommittedBoundaryResumeUnavailable::ArchivedNotRevivable {
                        runtime_state,
                    } => ResumeSessionLoad::ArchivedNotRevivable { runtime_state },
                };
                SessionResumeVerdict::from_authoritative_load_with_authority(
                    session_id,
                    load,
                    authority,
                    None,
                )
            }
            meerkat_session::PreparedCommittedBoundaryResume::CommittedBoundaryUnprovable {
                observation,
                reason,
            } => SessionResumeVerdict::committed_boundary_unprovable(
                session_id,
                ResumeSessionLoad::Absent,
                SessionResumeAuthority {
                    observation: Some(observation),
                },
                reason,
            ),
            meerkat_session::PreparedCommittedBoundaryResume::AuthorityChangedDuringMaterialization {
                observation,
            } => Ok(SessionResumeVerdict::authority_changed_during_materialization(
                session_id,
                SessionResumeAuthority {
                    observation: Some(observation),
                },
            )),
        }
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        resume_preparation: Option<SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let resume_preparation = match resume_preparation {
            Some(preparation) => {
                let session_id = req
                    .build
                    .as_ref()
                    .and_then(|build| build.resume_session.as_ref())
                    .map(|session| session.id())
                    .ok_or_else(|| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            "persistent actor materialization carried a resume preparation receipt without its session body"
                                .to_string(),
                        ))
                    })?;
                Some(preparation.into_persistent_for(session_id)?)
            }
            None => None,
        };
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_admission_and_actor_witness_under_runtime_turn_boundary(
            req,
            admission,
            resume_preparation,
            actor_witness_slot,
        )
        .await
    }

    #[cfg(feature = "experimental-gpt-live")]
    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        session_id: &SessionId,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        let (committed, store_commit_authority) =
            meerkat_session::PersistentSessionService::<B>::export_live_context_committed_boundary(
                self, session_id,
            )
            .await?;
        runtime_adapter
            .enqueue_committed_parent_session_boundary(
                session_id,
                &committed,
                &store_commit_authority,
            )
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to enqueue committed Mob parent-session boundary for {session_id}: {error}"
                )))
            })
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission(
            req,
            admission,
            authorization,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission_under_runtime_turn_boundary(
            req,
            admission,
            authorization,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        resume_preparation: SessionResumePreparationReceipt,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id())
            .ok_or_else(|| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "machine-authorized archived resume omitted its session body".to_string(),
                ))
            })?;
        let resume_preparation = resume_preparation.into_persistent_for(session_id)?;
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission_and_actor_witness_under_runtime_turn_boundary(
            req,
            admission,
            authorization,
            resume_preparation,
            actor_witness_slot,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn authorize_revivable_retired_session(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
        self.revive_archived_session_with_prepared_materialization(session_id, authority)
            .await
    }
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn persisted_session_authority_read_cost(&self) -> PersistedSessionAuthorityReadCost {
        match self.runtime_store().session_boundary_authority_read_cost() {
            meerkat_runtime::store::RuntimeSessionAuthorityReadCost::Bounded => {
                PersistedSessionAuthorityReadCost::Bounded
            }
            meerkat_runtime::store::RuntimeSessionAuthorityReadCost::Unsupported => {
                PersistedSessionAuthorityReadCost::Unsupported
            }
        }
    }

    async fn observe_persisted_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::identity::IdentitySessionStoreAuthority>, SessionError> {
        let authority =
            meerkat_session::PersistentSessionService::<B>::observe_persisted_session_authority(
                self, session_id,
            )
            .await?;
        Ok(authority.map(crate::identity::IdentitySessionStoreAuthority::from_runtime_authority))
    }

    async fn observe_session_resume_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        let observation = self
            .runtime_store()
            .load_session_resume_observation(&runtime_id)
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to atomically observe resume authority for session '{session_id}': {error}"
                )))
            })?;
        Ok(SessionResumeAuthority {
            observation: Some(observation),
        })
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(
            meerkat_session::PersistentSessionService::<B>::live_session_actor_registered(
                self, session_id,
            )
            .await,
        )
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let key = std::ptr::from_ref(self) as usize;
            let store = self.runtime_store();
            Some(cached_runtime_adapter(
                persistent_runtime_adapter_cache(),
                key,
                || {
                    Arc::new(meerkat_runtime::MeerkatMachine::persistent(
                        store,
                        self.blob_store(),
                    ))
                },
            ))
        }
    }

    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        true
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let Some(session) = self.load_authoritative_session(session_id).await? else {
            return Ok(None);
        };
        if self
            .session_archived_by_authority(session_id, &session)
            .await?
        {
            // The ordinary loader deliberately hides archived sessions; the
            // typed resume seam (`load_session_for_resume`) is where archived
            // stops reading as absent. Logged at debug because callers that
            // treat this None as "missing" are the misdiagnosis this line
            // exists to catch.
            tracing::debug!(
                session_id = %session_id,
                "load_persisted_session hid an archived session (use the resume seam for revival)"
            );
            return Ok(None);
        }
        Ok(Some(session))
    }

    async fn fork_persisted_session(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        meerkat_session::PersistentSessionService::<B>::fork_durable_session(
            self,
            source_session_id,
            message_count,
            tool_access_policy,
            Some(target),
        )
        .await
    }

    async fn load_revivable_retired_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        match self.load_session_for_resume(session_id).await? {
            ResumeSessionLoad::Revivable(session) => Ok(Some(*session)),
            // An ordinary active document is not this seam's business, and an
            // archived-but-refused one must not read as a plain absence here.
            ResumeSessionLoad::Active(_)
            | ResumeSessionLoad::ArchivedNotRevivable { .. }
            | ResumeSessionLoad::Absent => Ok(None),
        }
    }

    /// Canonical durable terminal first; the runtime record only proves
    /// quiescence. The admitted set is exactly what the downstream revival
    /// transaction accepts, so this seam cannot hand back a document the
    /// promotion step would then refuse.
    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        let Some(session) = self.observe_authoritative_session_body(session_id).await? else {
            return Ok(ResumeSessionLoad::Absent);
        };
        let runtime_state = self.persisted_runtime_state(session_id).await?;
        let store_archived = self
            .session_archived_by_authority(session_id, &session)
            .await?;
        // Current runtimes own lifecycle in RuntimeStore. An imported
        // pre-runtime document has no such row, so its machine-authored
        // terminal remains the only lifecycle authority until revival mints
        // the current runtime representation.
        let imported_document_archived = runtime_state.is_none()
            && session
                .lifecycle_terminal()
                .is_some_and(meerkat_core::SessionLifecycleTerminal::is_archived);
        let archived = store_archived || imported_document_archived;
        if !archived {
            // A Retired runtime over a non-archived document stays revivable,
            // preserving the pre-fix behaviour for that shape.
            if runtime_state == Some(meerkat_runtime::RuntimeState::Retired) {
                return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
            }
            return Ok(ResumeSessionLoad::Active(Box::new(session)));
        }
        match runtime_state {
            // Quiescent: no executor is attached and no run is in progress, so
            // the exact archived-resume lease cannot race a live writer.
            Some(meerkat_runtime::RuntimeState::Retired | meerkat_runtime::RuntimeState::Idle) => {
                Ok(ResumeSessionLoad::Revivable(Box::new(session)))
            }
            // Pre-runtime-store imports can carry a machine-authored archived
            // document without any runtime row. Absence of runtime authority
            // proves there is no attached executor to race; the retained
            // document is therefore the sole authority and may drive the
            // existing ReviveArchivedSessionDocument transition.
            None => Ok(ResumeSessionLoad::Revivable(Box::new(session))),
            // Live, attached, or running state is not quiescent. Refuse, but
            // say so truthfully.
            other => Ok(ResumeSessionLoad::ArchivedNotRevivable {
                runtime_state: other,
            }),
        }
    }

    /// Metadata-only authoritative read. Same visibility contract as
    /// [`Self::load_persisted_session`] — the archived filter runs through
    /// `session_archived_by_authority_with_terminal`, the terminal-fact
    /// sibling of the full-session overload, so archived sessions read as
    /// `None` on both seams.
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        let Some(view) = self.load_authoritative_session_metadata(session_id).await? else {
            return Ok(None);
        };
        if self
            .session_archived_by_authority_with_terminal(
                session_id,
                view.lifecycle_terminal.as_ref(),
            )
            .await?
        {
            return Ok(None);
        }
        Ok(Some(view))
    }

    async fn session_known_to_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        // Direct durable-carrier ownership read, deliberately NOT the
        // visibility-arbitrated / archived-filtered `load_persisted_session`:
        // an already-archived row and a deferred pre-first-turn store row are
        // both still OWNED by this authority. Only a session with neither a
        // store projection nor a runtime snapshot is host-owned.
        meerkat_session::PersistentSessionService::<B>::session_known_to_archive_authority(
            self, session_id,
        )
        .await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol(
                self,
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    runtime_adapter.as_ref(),
                ),
            )
            .await;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
        post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
    ) -> Result<(), SessionError> {
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol_under_runtime_turn_boundary_and_hook_before(
                self,
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    runtime_adapter.as_ref(),
                ),
                deadline,
                post_commit_hook,
            )
            .await;
        }
        if post_commit_hook.is_some() {
            return Err(SessionError::Unsupported(format!(
                "non-runtime persistent session {session_id} cannot run a pre-retire archive hook"
            )));
        }
        <Self as SessionService>::archive(self, session_id).await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol_under_runtime_turn_boundary(
                self,
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    runtime_adapter.as_ref(),
                ),
            )
            .await;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol_under_runtime_turn_boundary_before(
                self,
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    runtime_adapter.as_ref(),
                ),
                deadline,
            )
            .await;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::interrupt_with_machine_authority(
            self, session_id, authority,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::interrupt_run_with_machine_authority(
            self,
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::cancel_after_boundary_with_machine_authority(
            self, session_id, expected_run_id, authority,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::cancel_current_after_boundary_with_machine_authority(
            self, session_id, authority,
        )
        .await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::execution_snapshot(self, session_id).await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
            Err(error) => Err(SessionError::Unsupported(error.to_string())),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::PersistentSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::event_log_await_projection_drain(
            self, session_id,
        )
        .await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::PersistentSessionService::<B>::apply_runtime_turn(
            self,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn prepare_transient_turn_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        contexts: Vec<TurnRequestContext>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        meerkat_session::PersistentSessionService::<B>::prepare_live_transient_turn_context_boundary(
            self,
            session_id,
            expected_run_id,
            contexts,
        )
        .await
    }

    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::checkpoint_committed_runtime_session_snapshot(
            self,
            session_id,
            session_snapshot,
        )
        .await
    }

    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            meerkat_session::PersistentSessionService::<B>::acquire_runtime_turn_finalization_guard(
                self,
                session_id,
            )
            .await,
        ))
    }

    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
            self,
            session_id,
            session_snapshot,
        )
        .await
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::acknowledge_committed_runtime_session_boundary_under_runtime_turn_boundary(
            self,
            session_id,
            authority,
        )
        .await
    }

    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_after_runtime_stop_terminalized(
            self,
            session_id,
        )
        .await
    }

    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_after_runtime_stop_terminalized_under_runtime_turn_boundary(
            self,
            session_id,
        )
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_under_runtime_turn_boundary(
            self,
            session_id,
        )
        .await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_actor_under_runtime_turn_boundary(
            self, witness,
        )
        .await
    }

    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_actor_after_durability_reload_required(
            self, witness,
        )
        .await
    }

    async fn publish_interaction_terminals_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        meerkat_session::PersistentSessionService::<B>::publish_interaction_terminals_exact_batch_for_actor(
            self,
            actor_witness,
            events,
        )
        .await
    }

    async fn cancel_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::cancel_all_checkpointers(self).await;
    }

    async fn rearm_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::rearm_all_checkpointers(self).await;
    }
}
