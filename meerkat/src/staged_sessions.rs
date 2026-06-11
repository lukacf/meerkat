//! Staged session payload storage.
//!
//! Owns the "session ID claimed, build config staged, but not yet materialized
//! in the session service" state that the RPC/surface layers use for deferred
//! session creation. Generated MeerkatMachine DSL authority owns the lifecycle
//! phase and keep-alive policy; this registry stores the bulky build payloads
//! and applies them only after generated transitions accept.
//!
//! See `docs/wave-d-prep/d-j-staged-session-design.md` for the design
//! rationale. The facade crate owns this authority because the staged payload
//! carries [`AgentBuildConfig`], which is defined here and cannot be imported
//! by `meerkat-session` without inverting the crate dependency graph.
//!
//! The lifecycle:
//!
//! ```text
//!   ┌──────────┐  BeginDeferredSessionPromotion   ┌────────────┐
//!   │  Staged  │─────────────────────────────────▶│  Promoting │
//!   └──────────┘                                  └────────────┘
//!        ▲                                 │
//!        │ abandon_promotion()             │ finish_promotion() or abandon()
//!        └─────────────────────────────────┘
//!        │
//!        │ abandon() (explicit archive)
//!        ▼
//!      (removed)
//! ```

use meerkat_core::types::{ContentInput, SessionId};
use meerkat_core::{
    AppendSystemContextRequest, AppendSystemContextStatus, Session, SessionLlmIdentity,
    SessionSystemContextState,
};
use meerkat_runtime::meerkat_machine::dsl as machine_dsl;
use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::RwLock;

use crate::AgentBuildConfig;

/// Mechanical payload attached to a generated staged-session phase.
///
/// This enum must not decide lifecycle legality. Callers first apply the
/// corresponding generated MeerkatMachine input through
/// [`GeneratedStagedSessionAuthority`], then move payloads to mirror the
/// accepted generated phase.
enum StagedPayload {
    Staged {
        build_config: Box<AgentBuildConfig>,
    },
    Closing {
        build_config: Box<AgentBuildConfig>,
    },
    Promoting {
        starting_system_context_state: SessionSystemContextState,
        current_system_context_state: SessionSystemContextState,
    },
}

struct GeneratedStagedSessionAuthority {
    authority: machine_dsl::MeerkatMachineAuthority,
}

impl GeneratedStagedSessionAuthority {
    fn new(
        id: &SessionId,
        keep_alive: bool,
        has_comms_name: bool,
        effective_llm_identity: &SessionLlmIdentity,
        machine_archived_resume_authorized: bool,
    ) -> Result<Self, StagedLifecycleError> {
        let mut this = Self {
            authority: machine_dsl::MeerkatMachineAuthority::new(),
        };
        Self::map_keep_alive_rejection(
            this.apply(
                machine_dsl::MeerkatMachineInput::StageDeferredSession {
                    session_id: machine_dsl::SessionId::from_domain(id),
                    keep_alive,
                    has_comms_name,
                    llm_identity: machine_dsl::SessionLlmIdentity::from_domain(
                        effective_llm_identity,
                    ),
                    machine_archived_resume_authorized,
                },
                "StageDeferredSession",
            ),
            keep_alive,
            has_comms_name,
        )?;
        Ok(this)
    }

    fn apply(
        &mut self,
        input: machine_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), StagedLifecycleError> {
        machine_dsl::MeerkatMachineMutator::apply(&mut self.authority, input)
            .map(|_| ())
            .map_err(|err| {
                StagedLifecycleError::GeneratedAuthorityRejected {
                    context,
                    reason: match err {
                        machine_dsl::MeerkatMachineTransitionError::NoMatchingTransition {
                            phase,
                            trigger,
                        } => {
                            format!("no matching transition from {phase:?} for {trigger}")
                        }
                        machine_dsl::MeerkatMachineTransitionError::GuardRejected {
                            phase,
                            trigger,
                        } => {
                            format!("guard rejected transition from {phase:?} for {trigger}")
                        }
                        machine_dsl::MeerkatMachineTransitionError::RecoveredStateInvariantRejected {
                            phase,
                            invariant,
                        } => {
                            format!("recovered state violated invariant {invariant} in phase {phase:?}")
                        }
                    },
                }
            })
    }

    fn phase(&self) -> machine_dsl::StagedSessionPhase {
        self.authority.state().staged_session_phase
    }

    fn keep_alive(&self) -> Option<bool> {
        self.authority.state().staged_session_keep_alive
    }

    fn machine_archived_resume_authorized(&self) -> bool {
        self.authority
            .state()
            .staged_session_machine_archived_resume_authorized
    }

    fn effective_llm_identity(
        &self,
        context: &'static str,
    ) -> Result<SessionLlmIdentity, StagedLifecycleError> {
        let identity = self
            .authority
            .state()
            .staged_session_llm_identity
            .clone()
            .ok_or_else(|| StagedLifecycleError::GeneratedIdentityUnavailable {
                context,
                reason: "generated staged-session authority has no LLM identity".to_string(),
            })?;
        SessionLlmIdentity::try_from(identity).map_err(|reason| {
            StagedLifecycleError::GeneratedIdentityUnavailable { context, reason }
        })
    }

    fn update_keep_alive(
        &mut self,
        id: &SessionId,
        keep_alive: bool,
        has_comms_name: bool,
    ) -> Result<(), StagedLifecycleError> {
        Self::map_keep_alive_rejection(
            self.apply(
                machine_dsl::MeerkatMachineInput::UpdateDeferredSessionKeepAlive {
                    session_id: machine_dsl::SessionId::from_domain(id),
                    keep_alive,
                    has_comms_name,
                },
                "UpdateDeferredSessionKeepAlive",
            ),
            keep_alive,
            has_comms_name,
        )
    }

    fn update_llm_identity(
        &mut self,
        id: &SessionId,
        effective_llm_identity: &SessionLlmIdentity,
    ) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::UpdateDeferredSessionLlmIdentity {
                session_id: machine_dsl::SessionId::from_domain(id),
                llm_identity: machine_dsl::SessionLlmIdentity::from_domain(effective_llm_identity),
            },
            "UpdateDeferredSessionLlmIdentity",
        )
    }

    fn authorize_system_context_append(
        &mut self,
        id: &SessionId,
    ) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::AuthorizeDeferredSessionSystemContextAppend {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "AuthorizeDeferredSessionSystemContextAppend",
        )
    }

    fn map_keep_alive_rejection(
        result: Result<(), StagedLifecycleError>,
        keep_alive: bool,
        has_comms_name: bool,
    ) -> Result<(), StagedLifecycleError> {
        match result {
            Err(StagedLifecycleError::GeneratedAuthorityRejected { .. })
                if keep_alive && !has_comms_name =>
            {
                Err(StagedLifecycleError::KeepAliveRequiresCommsName)
            }
            other => other,
        }
    }

    fn begin_promotion(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::BeginDeferredSessionPromotion {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "BeginDeferredSessionPromotion",
        )
    }

    fn abandon_promotion(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::AbandonDeferredSessionPromotion {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "AbandonDeferredSessionPromotion",
        )
    }

    fn authorize_machine_archived_resume(
        &mut self,
        id: &SessionId,
    ) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::AuthorizeDeferredSessionMachineArchivedResume {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "AuthorizeDeferredSessionMachineArchivedResume",
        )
    }

    fn finish_promotion(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::FinishDeferredSessionPromotion {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "FinishDeferredSessionPromotion",
        )
    }

    fn begin_archive(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::BeginDeferredSessionArchive {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "BeginDeferredSessionArchive",
        )
    }

    fn restore_archive(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::RestoreDeferredSessionArchive {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "RestoreDeferredSessionArchive",
        )
    }

    fn finish_archive(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::FinishDeferredSessionArchive {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "FinishDeferredSessionArchive",
        )
    }

    fn drop_session(&mut self, id: &SessionId) -> Result<(), StagedLifecycleError> {
        self.apply(
            machine_dsl::MeerkatMachineInput::DropDeferredSession {
                session_id: machine_dsl::SessionId::from_domain(id),
            },
            "DropDeferredSession",
        )
    }
}

/// A staged session slot: generated phase authority + build payload +
/// surfaceable metadata.
pub struct StagedSlot {
    authority: GeneratedStagedSessionAuthority,
    payload: StagedPayload,
    pub labels: Option<BTreeMap<String, String>>,
    /// Prompt supplied at `session/create` time when the initial turn is
    /// deferred. Prepended to the first `turn/start` prompt on promotion.
    pub deferred_prompt: Option<ContentInput>,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
}

impl StagedSlot {
    #[allow(clippy::too_many_arguments)]
    pub fn new_staged(
        id: &SessionId,
        build_config: AgentBuildConfig,
        effective_llm_identity: SessionLlmIdentity,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
        created_at_secs: u64,
        updated_at_secs: u64,
        machine_archived_resume_authorized: bool,
    ) -> Result<Self, StagedLifecycleError> {
        let keep_alive = build_config.keep_alive;
        let has_comms_name = build_config.comms_name.is_some();
        Ok(Self {
            authority: GeneratedStagedSessionAuthority::new(
                id,
                keep_alive,
                has_comms_name,
                &effective_llm_identity,
                machine_archived_resume_authorized,
            )?,
            payload: StagedPayload::Staged {
                build_config: Box::new(build_config),
            },
            labels,
            deferred_prompt,
            created_at_secs,
            updated_at_secs,
        })
    }
}

/// Plain-old-data summary of a staged slot suitable for list/read surfaces.
#[derive(Clone)]
pub struct StagedSessionInfo {
    pub labels: BTreeMap<String, String>,
    pub effective_llm_identity: SessionLlmIdentity,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
    pub is_promoting: bool,
}

/// Slot payload returned by `begin_promotion` — the caller receives the
/// full build config plus the metadata it needs to either finish the
/// promotion path or restore the slot via `abandon_promotion`.
pub struct PromotingSlot {
    pub build_config: Box<AgentBuildConfig>,
    pub effective_llm_identity: SessionLlmIdentity,
    pub labels: Option<BTreeMap<String, String>>,
    pub deferred_prompt: Option<ContentInput>,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
    pub generated_machine_archived_resume_admission: GeneratedMachineArchivedResumeAdmission,
}

/// Typed handoff from generated staged-session authority to promotion
/// materialization. Callers may carry it to the service spawn path, but only
/// this module can construct it from generated machine state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedMachineArchivedResumeAdmission {
    authorized_by_generated_authority: bool,
}

impl GeneratedMachineArchivedResumeAdmission {
    fn from_generated_authority(authority: &GeneratedStagedSessionAuthority) -> Self {
        Self {
            authorized_by_generated_authority: authority.machine_archived_resume_authorized(),
        }
    }

    pub fn is_authorized_by_generated_authority(self) -> bool {
        self.authorized_by_generated_authority
    }
}

/// Errors returned by registry transitions.
#[derive(Debug, thiserror::Error)]
pub enum StagedLifecycleError {
    /// `stage` called for a session that is already staged.
    #[error("session already staged: {0}")]
    AlreadyStaged(SessionId),
    /// `begin_promotion` called while another promotion is already in flight.
    #[error("session already being promoted: {0}")]
    AlreadyPromoting(SessionId),
    /// No staged slot exists for the session.
    #[error("staged session not found: {0}")]
    NotFound(SessionId),
    /// A staged keep-alive override requires a comms participant name.
    #[error("keep_alive requires a session created with comms_name")]
    KeepAliveRequiresCommsName,
    /// Generated MeerkatMachine staged-session authority rejected a transition.
    #[error("generated staged-session authority rejected {context}: {reason}")]
    GeneratedAuthorityRejected {
        context: &'static str,
        reason: String,
    },
    /// Generated MeerkatMachine staged-session authority did not provide a
    /// usable machine-owned LLM identity.
    #[error("generated staged-session identity unavailable during {context}: {reason}")]
    GeneratedIdentityUnavailable {
        context: &'static str,
        reason: String,
    },
}

/// Payload registry for generated-authorized staged sessions.
///
/// Surfaces hold an `Arc<StagedSessionRegistry>` and route every
/// staged-session concern through this object. Phase and keep-alive policy
/// transitions are accepted by generated MeerkatMachine authority before the
/// registry moves payloads.
#[derive(Default)]
pub struct StagedSessionRegistry {
    slots: RwLock<HashMap<SessionId, StagedSlot>>,
}

impl StagedSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of staged sessions currently registered.
    pub async fn len(&self) -> usize {
        self.slots.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.slots.read().await.is_empty()
    }

    /// Whether any slot exists for `id`, regardless of phase.
    pub async fn contains(&self, id: &SessionId) -> bool {
        self.slots.read().await.contains_key(id)
    }

    /// Return a summary of the slot if present, failing closed if generated
    /// authority cannot provide the staged LLM identity.
    pub async fn info(
        &self,
        id: &SessionId,
    ) -> Result<Option<StagedSessionInfo>, StagedLifecycleError> {
        let slots = self.slots.read().await;
        slots.get(id).map(Self::slot_info).transpose()
    }

    /// Display-only projection for surfaces that cannot express staged
    /// projection errors in their legacy return shape.
    pub async fn project_info(&self, id: &SessionId) -> Option<StagedSessionInfo> {
        match self.info(id).await {
            Ok(info) => info,
            Err(error) => {
                tracing::warn!(
                    session_id = %id,
                    error = %error,
                    "failed to project staged-session info from generated authority"
                );
                None
            }
        }
    }

    /// Alias for callers that want explicit fallible projection semantics.
    pub async fn try_info(
        &self,
        id: &SessionId,
    ) -> Result<Option<StagedSessionInfo>, StagedLifecycleError> {
        self.info(id).await
    }

    /// Effective LLM identity captured by generated authority when the slot was
    /// staged.
    pub async fn effective_llm_identity(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionLlmIdentity>, StagedLifecycleError> {
        let slots = self.slots.read().await;
        slots
            .get(id)
            .map(|s| s.authority.effective_llm_identity("effective_llm_identity"))
            .transpose()
    }

    /// Keep-alive policy captured in the staged build config before the
    /// session is materialized into the persistent service.
    pub async fn keep_alive(&self, id: &SessionId) -> Option<bool> {
        let slots = self.slots.read().await;
        let slot = slots.get(id)?;
        slot.authority.keep_alive()
    }

    /// Apply a keep-alive override to a staged session before its first turn
    /// materializes it in the session service.
    pub async fn update_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
        updated_at_secs: u64,
    ) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        match &mut slot.payload {
            StagedPayload::Staged { build_config } => {
                slot.authority.update_keep_alive(
                    id,
                    keep_alive,
                    build_config.comms_name.is_some(),
                )?;
                build_config.keep_alive = keep_alive;
                slot.updated_at_secs = updated_at_secs;
                Ok(true)
            }
            StagedPayload::Promoting { .. } | StagedPayload::Closing { .. } => {
                Err(StagedLifecycleError::AlreadyPromoting(id.clone()))
            }
        }
    }

    /// Authorize a keep-alive override while the staged payload is already
    /// promoting. The caller owns the temporary build config and updates that
    /// payload only after this generated transition accepts.
    pub async fn authorize_promoting_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
        has_comms_name: bool,
    ) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        match &mut slot.payload {
            StagedPayload::Promoting { .. } => {
                slot.authority
                    .update_keep_alive(id, keep_alive, has_comms_name)?;
                Ok(true)
            }
            StagedPayload::Staged { .. } => Err(StagedLifecycleError::GeneratedAuthorityRejected {
                context: "UpdateDeferredSessionKeepAlive",
                reason: "payload is not in generated promoting phase".to_string(),
            }),
            StagedPayload::Closing { .. } => {
                Err(StagedLifecycleError::AlreadyPromoting(id.clone()))
            }
        }
    }

    /// Update the generated staged-session LLM identity before callers mutate
    /// the promoting build payload that will materialize the session.
    pub async fn update_effective_llm_identity(
        &self,
        id: &SessionId,
        effective_llm_identity: &SessionLlmIdentity,
    ) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        match &mut slot.payload {
            StagedPayload::Staged { .. } | StagedPayload::Promoting { .. } => {
                slot.authority
                    .update_llm_identity(id, effective_llm_identity)?;
                Ok(true)
            }
            StagedPayload::Closing { .. } => {
                Err(StagedLifecycleError::AlreadyPromoting(id.clone()))
            }
        }
    }

    /// Record, through generated authority, that the promoting materialized
    /// session was archived by machine control and may be retried once.
    pub async fn authorize_machine_archived_resume(
        &self,
        id: &SessionId,
    ) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        if !matches!(slot.payload, StagedPayload::Promoting { .. }) {
            return Err(StagedLifecycleError::AlreadyPromoting(id.clone()));
        }
        slot.authority.authorize_machine_archived_resume(id)?;
        Ok(true)
    }

    /// All slots, optionally filtered by labels, as (id, info) pairs.
    pub async fn list(
        &self,
        label_filter: Option<&BTreeMap<String, String>>,
    ) -> Vec<(SessionId, StagedSessionInfo)> {
        let slots = self.slots.read().await;
        slots
            .iter()
            .filter(|(_, slot)| Self::matches_label_filter(slot.labels.as_ref(), label_filter))
            .filter_map(|(id, slot)| match Self::slot_info(slot) {
                Ok(info) => Some((id.clone(), info)),
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        error = %error,
                        "skipping staged-session projection without generated identity"
                    );
                    None
                }
            })
            .collect()
    }

    /// Insert a newly-staged slot. Errors if a slot already exists for `id`.
    pub async fn stage(&self, id: SessionId, slot: StagedSlot) -> Result<(), StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        if slots.contains_key(&id) {
            return Err(StagedLifecycleError::AlreadyStaged(id));
        }
        slots.insert(id, slot);
        Ok(())
    }

    /// Flip a slot from `Staged` to `Promoting` and return the build
    /// payload + metadata. Called at the start of the pending-session
    /// materialization path.
    pub async fn begin_promotion(
        &self,
        id: &SessionId,
    ) -> Result<Option<PromotingSlot>, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(None);
        };
        let starting_state = match &slot.payload {
            StagedPayload::Staged { build_config } => build_config
                .resume_session
                .as_ref()
                .and_then(Session::system_context_state)
                .unwrap_or_default(),
            StagedPayload::Promoting { .. } | StagedPayload::Closing { .. } => {
                return Err(StagedLifecycleError::AlreadyPromoting(id.clone()));
            }
        };
        slot.authority.begin_promotion(id)?;
        let payload = std::mem::replace(
            &mut slot.payload,
            StagedPayload::Promoting {
                starting_system_context_state: starting_state.clone(),
                current_system_context_state: starting_state,
            },
        );
        let StagedPayload::Staged { build_config } = payload else {
            return Err(StagedLifecycleError::GeneratedAuthorityRejected {
                context: "BeginDeferredSessionPromotion",
                reason: "payload shape diverged from generated staged phase".to_string(),
            });
        };
        let effective_llm_identity = slot
            .authority
            .effective_llm_identity("BeginDeferredSessionPromotion")?;
        Ok(Some(PromotingSlot {
            build_config,
            effective_llm_identity,
            labels: slot.labels.clone(),
            deferred_prompt: slot.deferred_prompt.clone(),
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
            generated_machine_archived_resume_admission:
                GeneratedMachineArchivedResumeAdmission::from_generated_authority(&slot.authority),
        }))
    }

    /// Take and remove the system-context states for a slot that is
    /// `Promoting`. Used when the promotion has succeeded and the service
    /// has taken over: the caller replays staged system-context appends
    /// against the live session, then drops the slot entirely.
    pub async fn take_promoting_system_context_state(
        &self,
        id: &SessionId,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        let mut slots = self.slots.write().await;
        let should_remove = {
            let slot = slots.get_mut(id)?;
            if slot.authority.finish_promotion(id).is_err() {
                return None;
            }
            true
        };
        if !should_remove {
            return None;
        }
        let slot = slots.remove(id)?;
        match slot.payload {
            StagedPayload::Promoting {
                starting_system_context_state,
                current_system_context_state,
            } => Some((starting_system_context_state, current_system_context_state)),
            StagedPayload::Staged { .. } | StagedPayload::Closing { .. } => None,
        }
    }

    /// Clone the system-context states for a slot that is currently
    /// `Promoting` without removing the slot. Rollback paths use this before
    /// restoring the slot to `Staged`.
    pub async fn promoting_system_context_state(
        &self,
        id: &SessionId,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        let slots = self.slots.read().await;
        let slot = slots.get(id)?;
        match &slot.payload {
            StagedPayload::Promoting {
                starting_system_context_state,
                current_system_context_state,
            } => Some((
                starting_system_context_state.clone(),
                current_system_context_state.clone(),
            )),
            StagedPayload::Staged { .. } | StagedPayload::Closing { .. } => None,
        }
    }

    /// Restore a slot from `Promoting` back to `Staged` when override
    /// validation or service-side materialization fails before the first
    /// successful turn. Preserves the original metadata + deferred prompt
    /// so the next `start_turn` call sees the same pre-promotion state.
    #[allow(clippy::too_many_arguments)]
    pub async fn abandon_promotion(
        &self,
        id: SessionId,
        build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
        created_at_secs: u64,
        updated_at_secs: u64,
    ) -> bool {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(&id) else {
            return false;
        };
        if !matches!(slot.payload, StagedPayload::Promoting { .. }) {
            return false;
        }
        if slot.authority.abandon_promotion(&id).is_err() {
            return false;
        }
        let updated_at_secs = updated_at_secs.max(slot.updated_at_secs);
        slot.payload = StagedPayload::Staged {
            build_config: Box::new(build_config),
        };
        slot.labels = labels;
        slot.deferred_prompt = deferred_prompt;
        slot.created_at_secs = created_at_secs;
        slot.updated_at_secs = updated_at_secs;
        true
    }

    /// Mark a staged slot as closing without removing it. This makes archive
    /// admission observable to competing surface paths while durable cleanup
    /// is still fallible.
    pub async fn begin_archive(&self, id: &SessionId) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        match &mut slot.payload {
            StagedPayload::Staged { .. } => {
                slot.authority.begin_archive(id)?;
                let payload = std::mem::replace(
                    &mut slot.payload,
                    StagedPayload::Promoting {
                        starting_system_context_state: SessionSystemContextState::default(),
                        current_system_context_state: SessionSystemContextState::default(),
                    },
                );
                let StagedPayload::Staged { build_config } = payload else {
                    return Err(StagedLifecycleError::GeneratedAuthorityRejected {
                        context: "BeginDeferredSessionArchive",
                        reason: "payload shape diverged from generated archive phase".to_string(),
                    });
                };
                slot.payload = StagedPayload::Closing { build_config };
                Ok(true)
            }
            StagedPayload::Promoting { .. } | StagedPayload::Closing { .. } => {
                Err(StagedLifecycleError::AlreadyPromoting(id.clone()))
            }
        }
    }

    /// Restore a slot marked by `begin_archive` back to ordinary staged state.
    pub async fn restore_archive(&self, id: &SessionId) -> bool {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return false;
        };
        if !matches!(slot.payload, StagedPayload::Closing { .. }) {
            return false;
        }
        if slot.authority.restore_archive(id).is_err() {
            return false;
        }
        let payload = std::mem::replace(
            &mut slot.payload,
            StagedPayload::Promoting {
                starting_system_context_state: SessionSystemContextState::default(),
                current_system_context_state: SessionSystemContextState::default(),
            },
        );
        let StagedPayload::Closing { build_config } = payload else {
            return false;
        };
        slot.payload = StagedPayload::Staged { build_config };
        true
    }

    /// Remove a slot that is currently closing after durable archive succeeds.
    pub async fn finish_archive(&self, id: &SessionId) -> bool {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return false;
        };
        if !matches!(slot.payload, StagedPayload::Closing { .. }) {
            return false;
        }
        if slot.authority.finish_archive(id).is_err() {
            return false;
        }
        slots.remove(id);
        true
    }

    /// Drop a slot entirely (archive path).
    pub async fn abandon(&self, id: &SessionId) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(false);
        };
        slot.authority.drop_session(id)?;
        Ok(slots.remove(id).is_some())
    }

    /// Drop a slot only if it has not begun promotion. Archive callers use
    /// this to avoid releasing active capacity while the first turn is already
    /// materializing.
    pub async fn abandon_staged(&self, id: &SessionId) -> Result<bool, StagedLifecycleError> {
        let mut slots = self.slots.write().await;
        let Some(phase) = slots.get(id).map(|slot| slot.authority.phase()) else {
            return Ok(false);
        };
        match phase {
            machine_dsl::StagedSessionPhase::Staged => {
                let slot = slots
                    .get_mut(id)
                    .ok_or_else(|| StagedLifecycleError::NotFound(id.clone()))?;
                slot.authority.drop_session(id)?;
                slots.remove(id);
                Ok(true)
            }
            machine_dsl::StagedSessionPhase::Promoting
            | machine_dsl::StagedSessionPhase::Closing => {
                Err(StagedLifecycleError::AlreadyPromoting(id.clone()))
            }
            machine_dsl::StagedSessionPhase::NotStaged => Ok(false),
        }
    }

    /// Drop all slots (shutdown path).
    pub async fn clear(&self) {
        let mut slots = self.slots.write().await;
        slots.retain(|id, slot| slot.authority.drop_session(id).is_err());
    }

    /// Apply a system-context append against a staged or promoting slot.
    /// Mutates the slot in place and bumps `updated_at_secs`.
    ///
    /// `now_secs` is the caller-supplied Unix seconds for the mutation timestamp.
    /// `now_system_time` is the system time stamped on the pending append entry.
    pub async fn append_system_context(
        &self,
        id: &SessionId,
        req: &AppendSystemContextRequest,
        now_system_time: meerkat_core::time_compat::SystemTime,
        now_secs: u64,
    ) -> Option<Result<AppendSystemContextStatus, meerkat_core::SystemContextStageError>> {
        let mut slots = self.slots.write().await;
        let slot = slots.get_mut(id)?;
        let result = match &mut slot.payload {
            StagedPayload::Staged { build_config } => {
                if let Err(err) = slot.authority.authorize_system_context_append(id) {
                    return Some(Err(meerkat_core::SystemContextStageError::InvalidRequest(
                        format!(
                            "generated staged-session system-context authority rejected append: {err}"
                        ),
                    )));
                }
                let session = build_config
                    .resume_session
                    .get_or_insert_with(|| Session::with_id(id.clone()));
                let mut state = match session.try_system_context_state() {
                    Ok(state) => state.unwrap_or_default(),
                    Err(err) => {
                        return Some(Err(meerkat_core::SystemContextStageError::InvalidRequest(
                            format!(
                                "generated system-context authority rejected staged restore: {err}"
                            ),
                        )));
                    }
                };
                let stage_result = state.stage_append(req, now_system_time);
                match stage_result {
                    Ok(status) => match session.set_system_context_state(state) {
                        Ok(()) => Ok(status),
                        Err(_) => {
                            // Serialization failure — surface via a dedicated
                            // `InvalidRequest` since the callers currently map
                            // this to an internal error anyway. We preserve the
                            // existing contract by letting the caller treat a
                            // serialization failure as a post-stage problem.
                            Err(meerkat_core::SystemContextStageError::InvalidRequest(
                                "failed to serialize system-context state".to_string(),
                            ))
                        }
                    },
                    Err(e) => Err(e),
                }
            }
            StagedPayload::Promoting {
                current_system_context_state,
                ..
            } => {
                if let Err(err) = slot.authority.authorize_system_context_append(id) {
                    return Some(Err(meerkat_core::SystemContextStageError::InvalidRequest(
                        format!(
                            "generated staged-session system-context authority rejected append: {err}"
                        ),
                    )));
                }
                current_system_context_state.stage_append(req, now_system_time)
            }
            StagedPayload::Closing { .. } => {
                Err(meerkat_core::SystemContextStageError::InvalidRequest(
                    "session is being archived".to_string(),
                ))
            }
        };
        if result.is_ok() {
            slot.updated_at_secs = now_secs;
        }
        Some(result)
    }

    fn slot_info(slot: &StagedSlot) -> Result<StagedSessionInfo, StagedLifecycleError> {
        Ok(StagedSessionInfo {
            labels: slot.labels.clone().unwrap_or_default(),
            effective_llm_identity: slot.authority.effective_llm_identity("slot_info")?,
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
            is_promoting: matches!(
                slot.authority.phase(),
                machine_dsl::StagedSessionPhase::Promoting
                    | machine_dsl::StagedSessionPhase::Closing
            ),
        })
    }

    fn matches_label_filter(
        slot_labels: Option<&BTreeMap<String, String>>,
        filter: Option<&BTreeMap<String, String>>,
    ) -> bool {
        let Some(filter) = filter else {
            return true;
        };
        let Some(slot_labels) = slot_labels else {
            return filter.is_empty();
        };
        filter.iter().all(|(k, v)| slot_labels.get(k) == Some(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::Provider;

    fn identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn slot(id: &SessionId) -> StagedSlot {
        StagedSlot::new_staged(
            id,
            AgentBuildConfig::new("test-model".to_string()),
            identity("test-model"),
            None,
            None,
            100,
            100,
            false,
        )
        .expect("test staged slot should be accepted by generated authority")
    }

    #[tokio::test]
    async fn stage_then_promote_round_trip() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        assert!(reg.contains(&id).await);
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("Staged slot should promote on first call");
        assert_eq!(promoted.created_at_secs, 100);
        let info = reg.info(&id).await.unwrap().unwrap();
        assert!(info.is_promoting);
        // Dropping the slot from promoting state simulates a successful
        // materialization: registry no longer knows about the session.
        let states = reg.take_promoting_system_context_state(&id).await;
        assert!(states.is_some());
        assert!(!reg.contains(&id).await);
    }

    #[tokio::test]
    async fn stage_then_abandon_without_promote() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        assert!(reg.abandon(&id).await.unwrap());
        assert!(!reg.contains(&id).await);
    }

    #[tokio::test]
    async fn begin_promotion_twice_rejects_second_caller() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let first = reg.begin_promotion(&id).await;
        assert!(matches!(first, Ok(Some(_))));
        let second = reg.begin_promotion(&id).await;
        match second {
            Err(StagedLifecycleError::AlreadyPromoting(got)) => assert_eq!(got, id),
            Err(other) => panic!("expected AlreadyPromoting, got {other:?}"),
            Ok(_) => panic!("expected AlreadyPromoting, got Ok"),
        }
    }

    #[tokio::test]
    async fn begin_promotion_missing_returns_none() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        let result = reg.begin_promotion(&id).await;
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn generated_keep_alive_authority_requires_comms_name() {
        let id = SessionId::new();
        let mut build_config = AgentBuildConfig::new("test-model".to_string());
        build_config.keep_alive = true;
        let rejected = StagedSlot::new_staged(
            &id,
            build_config,
            identity("test-model"),
            None,
            None,
            100,
            100,
            false,
        );
        assert!(matches!(
            rejected,
            Err(StagedLifecycleError::KeepAliveRequiresCommsName)
        ));

        let reg = StagedSessionRegistry::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let err = reg.update_keep_alive(&id, true, 101).await.unwrap_err();
        assert!(matches!(
            err,
            StagedLifecycleError::KeepAliveRequiresCommsName
        ));
        assert_eq!(reg.keep_alive(&id).await, Some(false));
    }

    #[tokio::test]
    async fn abandon_promotion_restores_staged() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("initial promotion");
        assert!(reg.info(&id).await.unwrap().unwrap().is_promoting);
        reg.abandon_promotion(
            id.clone(),
            *promoted.build_config,
            promoted.labels,
            promoted.deferred_prompt,
            promoted.created_at_secs,
            promoted.updated_at_secs,
        )
        .await;
        // Slot must be Staged again so a follow-up start_turn sees the
        // pre-promotion state.
        assert!(!reg.info(&id).await.unwrap().unwrap().is_promoting);
        let promoted_again = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("re-promotion after abandon");
        assert_eq!(promoted_again.created_at_secs, 100);
    }

    #[tokio::test]
    async fn abandon_promotion_does_not_resurrect_finished_slot() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("initial promotion");
        assert!(reg.take_promoting_system_context_state(&id).await.is_some());
        let restored = reg
            .abandon_promotion(
                id.clone(),
                *promoted.build_config,
                promoted.labels,
                promoted.deferred_prompt,
                promoted.created_at_secs,
                promoted.updated_at_secs,
            )
            .await;
        assert!(!restored, "finished promotion must not be restored");
        assert!(!reg.contains(&id).await);
    }

    #[tokio::test]
    async fn begin_archive_blocks_promotion_until_restored_or_finished() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();

        assert!(reg.begin_archive(&id).await.unwrap());
        assert!(reg.info(&id).await.unwrap().unwrap().is_promoting);
        let promoting = reg.begin_promotion(&id).await;
        assert!(matches!(
            promoting,
            Err(StagedLifecycleError::AlreadyPromoting(_))
        ));

        assert!(reg.restore_archive(&id).await);
        assert!(!reg.info(&id).await.unwrap().unwrap().is_promoting);
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("promotion should succeed after archive restore");
        assert!(
            reg.abandon_promotion(
                id.clone(),
                *promoted.build_config,
                promoted.labels,
                promoted.deferred_prompt,
                promoted.created_at_secs,
                promoted.updated_at_secs,
            )
            .await
        );

        assert!(reg.begin_archive(&id).await.unwrap());
        assert!(reg.finish_archive(&id).await);
        assert!(!reg.contains(&id).await);
    }

    #[tokio::test]
    async fn stage_is_idempotent_guard() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let result = reg.stage(id.clone(), slot(&id)).await;
        assert!(matches!(
            result,
            Err(StagedLifecycleError::AlreadyStaged(_))
        ));
    }

    #[tokio::test]
    async fn append_system_context_mutates_staged_slot() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot(&id)).await.unwrap();
        let req = AppendSystemContextRequest {
            content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                "hello".to_string(),
            ),
            source: Some("test".to_string()),
            idempotency_key: Some("k1".to_string()),
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            peer_response_terminal: None,
        };
        let now = meerkat_core::time_compat::SystemTime::now();
        let res = reg.append_system_context(&id, &req, now, 200).await;
        let outcome = res.expect("slot is present");
        assert!(outcome.is_ok(), "staged append should succeed");
        let info = reg.info(&id).await.unwrap().unwrap();
        assert_eq!(info.updated_at_secs, 200);
    }

    #[tokio::test]
    async fn list_filters_by_labels() {
        let reg = StagedSessionRegistry::new();
        let id_a = SessionId::new();
        let id_b = SessionId::new();
        let mut slot_a = slot(&id_a);
        slot_a.labels = Some(BTreeMap::from([("env".to_string(), "prod".to_string())]));
        reg.stage(id_a.clone(), slot_a).await.unwrap();
        let mut slot_b = slot(&id_b);
        slot_b.labels = Some(BTreeMap::from([("env".to_string(), "dev".to_string())]));
        reg.stage(id_b.clone(), slot_b).await.unwrap();
        let filter = BTreeMap::from([("env".to_string(), "prod".to_string())]);
        let result = reg.list(Some(&filter)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, id_a);
    }
}
