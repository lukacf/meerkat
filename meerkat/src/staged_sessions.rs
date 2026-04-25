//! Staged session lifecycle authority.
//!
//! Owns the "session ID claimed, build config staged, but not yet materialized
//! in the session service" state that the RPC/surface layers use for deferred
//! session creation. The registry is the canonical owner of this state —
//! surfaces call through it instead of holding local phase discriminants.
//!
//! See `docs/wave-d-prep/d-j-staged-session-design.md` for the design
//! rationale. The facade crate owns this authority because the staged payload
//! carries [`AgentBuildConfig`], which is defined here and cannot be imported
//! by `meerkat-session` without inverting the crate dependency graph.
//!
//! The lifecycle:
//!
//! ```text
//!   ┌──────────┐  begin_promotion()   ┌────────────┐
//!   │  Staged  │─────────────────────▶│  Promoting │
//!   └──────────┘                      └────────────┘
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
use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::RwLock;

use crate::AgentBuildConfig;

/// Phase discriminant for a staged session.
pub enum StagedPhase {
    /// Session ID claimed, build config staged, first turn has not started.
    Staged { build_config: Box<AgentBuildConfig> },
    /// `start_turn` is running; the build config has been consumed and the
    /// session is being materialized in the service. Concurrent `start_turn`
    /// calls on the same ID are rejected with `AlreadyPromoting`.
    Promoting {
        starting_system_context_state: SessionSystemContextState,
        current_system_context_state: SessionSystemContextState,
    },
}

/// A staged session slot: build payload + surfaceable metadata.
pub struct StagedSlot {
    pub phase: StagedPhase,
    pub effective_llm_identity: SessionLlmIdentity,
    pub labels: Option<BTreeMap<String, String>>,
    /// Prompt supplied at `session/create` time when the initial turn is
    /// deferred. Prepended to the first `turn/start` prompt on promotion.
    pub deferred_prompt: Option<ContentInput>,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
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
    pub labels: Option<BTreeMap<String, String>>,
    pub deferred_prompt: Option<ContentInput>,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
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
}

/// Canonical staged-session registry.
///
/// Surfaces hold an `Arc<StagedSessionRegistry>` and route every
/// staged-session concern (existence, phase, metadata, system-context
/// mutation, promotion, abandonment) through this object.
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

    /// Return a summary of the slot if present.
    pub async fn info(&self, id: &SessionId) -> Option<StagedSessionInfo> {
        let slots = self.slots.read().await;
        slots.get(id).map(Self::slot_info)
    }

    /// Effective LLM identity captured when the slot was staged.
    pub async fn effective_llm_identity(&self, id: &SessionId) -> Option<SessionLlmIdentity> {
        let slots = self.slots.read().await;
        slots.get(id).map(|s| s.effective_llm_identity.clone())
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
            .map(|(id, slot)| (id.clone(), Self::slot_info(slot)))
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
        let starting_state = match &slot.phase {
            StagedPhase::Staged { build_config } => build_config
                .resume_session
                .as_ref()
                .and_then(Session::system_context_state)
                .unwrap_or_default(),
            StagedPhase::Promoting { .. } => {
                return Err(StagedLifecycleError::AlreadyPromoting(id.clone()));
            }
        };
        let phase = std::mem::replace(
            &mut slot.phase,
            StagedPhase::Promoting {
                starting_system_context_state: starting_state.clone(),
                current_system_context_state: starting_state,
            },
        );
        let StagedPhase::Staged { build_config } = phase else {
            unreachable!("phase was checked before replacement");
        };
        Ok(Some(PromotingSlot {
            build_config,
            labels: slot.labels.clone(),
            deferred_prompt: slot.deferred_prompt.clone(),
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
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
        let slot = slots.remove(id)?;
        match slot.phase {
            StagedPhase::Promoting {
                starting_system_context_state,
                current_system_context_state,
            } => Some((starting_system_context_state, current_system_context_state)),
            StagedPhase::Staged { build_config } => {
                // Should not happen on the success path; put it back.
                slots.insert(
                    id.clone(),
                    StagedSlot {
                        phase: StagedPhase::Staged { build_config },
                        effective_llm_identity: slot.effective_llm_identity,
                        labels: slot.labels,
                        deferred_prompt: slot.deferred_prompt,
                        created_at_secs: slot.created_at_secs,
                        updated_at_secs: slot.updated_at_secs,
                    },
                );
                None
            }
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
        effective_llm_identity: SessionLlmIdentity,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
        created_at_secs: u64,
        updated_at_secs: u64,
    ) {
        let mut slots = self.slots.write().await;
        slots.insert(
            id,
            StagedSlot {
                phase: StagedPhase::Staged {
                    build_config: Box::new(build_config),
                },
                effective_llm_identity,
                labels,
                deferred_prompt,
                created_at_secs,
                updated_at_secs,
            },
        );
    }

    /// Drop a slot entirely (archive path).
    pub async fn abandon(&self, id: &SessionId) -> bool {
        self.slots.write().await.remove(id).is_some()
    }

    /// Drop all slots (shutdown path).
    pub async fn clear(&self) {
        self.slots.write().await.clear();
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
        let result = match &mut slot.phase {
            StagedPhase::Staged { build_config } => {
                let session = build_config
                    .resume_session
                    .get_or_insert_with(|| Session::with_id(id.clone()));
                let mut state = session.system_context_state().unwrap_or_default();
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
            StagedPhase::Promoting {
                current_system_context_state,
                ..
            } => current_system_context_state.stage_append(req, now_system_time),
        };
        if result.is_ok() {
            slot.updated_at_secs = now_secs;
        }
        Some(result)
    }

    fn slot_info(slot: &StagedSlot) -> StagedSessionInfo {
        StagedSessionInfo {
            labels: slot.labels.clone().unwrap_or_default(),
            effective_llm_identity: slot.effective_llm_identity.clone(),
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
            is_promoting: matches!(slot.phase, StagedPhase::Promoting { .. }),
        }
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
            connection_ref: None,
        }
    }

    fn slot() -> StagedSlot {
        StagedSlot {
            phase: StagedPhase::Staged {
                build_config: Box::new(AgentBuildConfig::new("test-model".to_string())),
            },
            effective_llm_identity: identity("test-model"),
            labels: None,
            deferred_prompt: None,
            created_at_secs: 100,
            updated_at_secs: 100,
        }
    }

    #[tokio::test]
    async fn stage_then_promote_round_trip() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot()).await.unwrap();
        assert!(reg.contains(&id).await);
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("Staged slot should promote on first call");
        assert_eq!(promoted.created_at_secs, 100);
        let info = reg.info(&id).await.unwrap();
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
        reg.stage(id.clone(), slot()).await.unwrap();
        assert!(reg.abandon(&id).await);
        assert!(!reg.contains(&id).await);
    }

    #[tokio::test]
    async fn begin_promotion_twice_rejects_second_caller() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot()).await.unwrap();
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
    async fn abandon_promotion_restores_staged() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot()).await.unwrap();
        let promoted = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("initial promotion");
        assert!(reg.info(&id).await.unwrap().is_promoting);
        reg.abandon_promotion(
            id.clone(),
            *promoted.build_config,
            identity("test-model"),
            promoted.labels,
            promoted.deferred_prompt,
            promoted.created_at_secs,
            promoted.updated_at_secs,
        )
        .await;
        // Slot must be Staged again so a follow-up start_turn sees the
        // pre-promotion state.
        assert!(!reg.info(&id).await.unwrap().is_promoting);
        let promoted_again = reg
            .begin_promotion(&id)
            .await
            .ok()
            .flatten()
            .expect("re-promotion after abandon");
        assert_eq!(promoted_again.created_at_secs, 100);
    }

    #[tokio::test]
    async fn stage_is_idempotent_guard() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot()).await.unwrap();
        let result = reg.stage(id.clone(), slot()).await;
        assert!(matches!(
            result,
            Err(StagedLifecycleError::AlreadyStaged(_))
        ));
    }

    #[tokio::test]
    async fn append_system_context_mutates_staged_slot() {
        let reg = StagedSessionRegistry::new();
        let id = SessionId::new();
        reg.stage(id.clone(), slot()).await.unwrap();
        let req = AppendSystemContextRequest {
            text: "hello".to_string(),
            source: Some("test".to_string()),
            idempotency_key: Some("k1".to_string()),
        };
        let now = meerkat_core::time_compat::SystemTime::now();
        let res = reg.append_system_context(&id, &req, now, 200).await;
        let outcome = res.expect("slot is present");
        assert!(outcome.is_ok(), "staged append should succeed");
        let info = reg.info(&id).await.unwrap();
        assert_eq!(info.updated_at_secs, 200);
    }

    #[tokio::test]
    async fn list_filters_by_labels() {
        let reg = StagedSessionRegistry::new();
        let id_a = SessionId::new();
        let id_b = SessionId::new();
        let mut slot_a = slot();
        slot_a.labels = Some(BTreeMap::from([("env".to_string(), "prod".to_string())]));
        reg.stage(id_a.clone(), slot_a).await.unwrap();
        let mut slot_b = slot();
        slot_b.labels = Some(BTreeMap::from([("env".to_string(), "dev".to_string())]));
        reg.stage(id_b.clone(), slot_b).await.unwrap();
        let filter = BTreeMap::from([("env".to_string(), "prod".to_string())]);
        let result = reg.list(Some(&filter)).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, id_a);
    }
}
