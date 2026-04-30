#[cfg(feature = "runtime-adapter")]
use super::bridge::MobBoundMemberRuntimeBridge;
#[cfg(feature = "runtime-adapter")]
use super::local_bridge::LocalMobRuntimeBridge;
use super::*;
use crate::RuntimeBinding;
use crate::definition::ExternalBackendConfig;
use crate::event::MemberRef;
use crate::runtime::handle::MemberSpawnReceipt;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::event_injector::SubscribableInjector;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorControl, CoreExecutorError,
};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::run_control::RunControlCommand;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::{InputId, RunId as CoreRunId};
use meerkat_core::ops::OperationId;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::ops_lifecycle::OperationStatus;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{CreateSessionRequest, SessionError, StartTurnRequest};
use meerkat_core::types::SessionId;
#[cfg(feature = "runtime-adapter")]
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
#[cfg(feature = "runtime-adapter")]
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, MeerkatMachine, PromptInput,
};
#[cfg(feature = "runtime-adapter")]
use serde::de::DeserializeOwned;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::time::Duration;
#[cfg(feature = "runtime-adapter")]
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

type TurnEventTx = tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

pub struct ProvisionMemberRequest {
    pub create_session: CreateSessionRequest,
    pub binding: RuntimeBinding,
    pub peer_name: String,
    pub(crate) owner_bridge_session_id: Option<SessionId>,
    pub ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobProvisioner: Send + Sync {
    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError>;
    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError>;
    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &crate::RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let _ = (run_id, step_id);
        self.start_turn(member_ref, req).await
    }
    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>>;
    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError>;
    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let _ = member_ref;
        Ok(())
    }
    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>>;
    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError>;
    async fn reconcile_peer_only_trust(
        &self,
        member_ref: &MemberRef,
        desired_specs: &[TrustedPeerDescriptor],
    ) -> Result<(), MobError> {
        let _ = (member_ref, desired_specs);
        Ok(())
    }
    /// Resolve the live canonical mob-child lifecycle operation for an
    /// existing member bridge.
    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId>;
    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError>;

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

#[cfg(feature = "runtime-adapter")]
pub struct SessionBackend {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<MeerkatMachine>>,
    ops_adapter: Arc<super::ops_adapter::MobOpsAdapter>,
    // Capability index for runtime bridge sidecars keyed by registered runtime
    // session identity. This map is never lifecycle truth; canonical
    // registration/attachment truth stays in MeerkatMachine.
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

#[cfg(feature = "runtime-adapter")]
fn stamp_eager_session_owned_initial_turn_metadata(req: &mut CreateSessionRequest) {
    if req.initial_turn != meerkat_core::service::InitialTurnPolicy::RunImmediately {
        return;
    }
    let build = req
        .build
        .get_or_insert_with(meerkat_core::service::SessionBuildOptions::default);
    build.initial_turn_metadata = Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
        build.initial_turn_metadata.take(),
    ));
}

#[cfg(feature = "runtime-adapter")]
impl SessionBackend {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            ops_adapter: Arc::new(super::ops_adapter::MobOpsAdapter::new()),
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn require_session(
        member_ref: &MemberRef,
        operation: &'static str,
    ) -> Result<SessionId, MobError> {
        member_ref.bridge_session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "session-backed provisioner cannot {operation} member without session bridge: {member_ref:?}"
            ))
        })
    }

    fn trusted_peer_spec(
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        // Post-#31: real comms runtimes return `"ed25519:<base64>"` from
        // `CommsRuntime::public_key()` (see the trait doc at
        // `meerkat-core/src/agent.rs::CommsRuntime::public_key`). Decode it
        // back to the 32-byte Ed25519 pubkey and derive the UUIDv5
        // `PeerId` so we can stamp both onto the descriptor — otherwise
        // the supervisor's trust store entry holds a zero-pubkey row and
        // the external peer's real-keyed signed envelopes fail admission
        // at ingress with `UntrustedSender`.
        //
        // Pre-#24 `PeerId::parse` accepted arbitrary strings (including
        // the `"ed25519:…"` form); post-#24 it only accepts hyphenated
        // UUIDs. Some legacy test/runtime shims still pass a UUID-shaped
        // peer_id directly instead of an Ed25519 public-key carrier, so we
        // fall back to the legacy `test_only_unsigned` zero-pubkey descriptor
        // for non-ed25519 strings. Those paths remain inproc-only —
        // signature verification is bypassed there, so a zero-pubkey
        // descriptor is admission-safe.
        if let Some(pubkey_b64) = fallback_peer_id.strip_prefix("ed25519:") {
            let _ = pubkey_b64; // pattern anchor; full parse delegates below.
            let pubkey =
                meerkat_comms::PubKey::from_pubkey_string(fallback_peer_id).map_err(|err| {
                    MobError::WiringError(format!(
                        "invalid peer spec: invalid pubkey string for '{fallback_name}': {err}"
                    ))
                })?;
            let derived_peer_id = pubkey.to_peer_id().to_string();
            return TrustedPeerDescriptor::unsigned_with_pubkey(
                fallback_name,
                derived_peer_id,
                *pubkey.as_bytes(),
                format!("inproc://{fallback_name}"),
            )
            .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")));
        }
        TrustedPeerDescriptor::test_only_unsigned(
            fallback_name,
            fallback_peer_id,
            format!("inproc://{fallback_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
    }

    async fn runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<RuntimeSessionState>> {
        let adapter = self.runtime_adapter.as_ref()?;
        if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned() {
            if adapter.session_has_executor(session_id).await {
                return Some(existing);
            }
            existing.clear_queued_turns().await;
            let state = Arc::new(RuntimeSessionState {
                queued_turns: Mutex::new(RuntimeSessionQueue::default()),
            });
            let executor = Box::new(MobSessionRuntimeExecutor::new(
                self.session_service.clone(),
                Arc::clone(adapter),
                session_id.clone(),
                state.clone(),
                Arc::clone(&self.runtime_sessions),
            ));
            // Runtime session registrations are capability bindings. If the
            // adapter lost this session, drop any queued bridge context from
            // the stale binding before reattaching with a fresh sidecar.
            adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
            self.runtime_sessions
                .write()
                .await
                .insert(session_id.clone(), state.clone());
            return Some(state);
        }
        let state = Arc::new(RuntimeSessionState {
            queued_turns: Mutex::new(RuntimeSessionQueue::default()),
        });
        let executor = Box::new(MobSessionRuntimeExecutor::new(
            self.session_service.clone(),
            Arc::clone(adapter),
            session_id.clone(),
            state.clone(),
            Arc::clone(&self.runtime_sessions),
        ));
        adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        Some(state)
    }

    async fn remove_runtime_session_state(&self, session_id: &SessionId) {
        let removed = self.runtime_sessions.write().await.remove(session_id);
        if let Some(state) = removed {
            state.clear_queued_turns().await;
        }
    }

    async fn execute_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<TurnEventTx>,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self.runtime_session_state(session_id).await;
        let adapter_session_id = session_id.clone();
        let requested_input_id = input.id().clone();
        let mut context_input_id = requested_input_id.clone();

        // Queue only owner bridge context that cannot be reconstructed from
        // runtime primitives. Bind context by canonical input identity (not by
        // FIFO order) so runtime-owned contributing IDs remain the sole source
        // of semantic ordering.
        let queued_context = if let Some(ref state) = state {
            state
                .enqueue_turn_context(requested_input_id.clone(), event_tx)
                .await
        } else {
            false
        };

        let (outcome, handle) = match adapter
            .accept_input_with_completion(&adapter_session_id, input)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let Some(state) = state.as_ref()
                    && queued_context
                {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                return Err(MobError::Internal(err.to_string()));
            }
        };

        if let Some(state) = state.as_ref()
            && queued_context
        {
            let canonical_input_id = match &outcome {
                meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                // For in-flight dedup, runtime primitives are keyed by the existing
                // canonical input id, not the newly attempted one.
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    Some(existing_id)
                }
                _ => None,
            };
            if let Some(input_id) = canonical_input_id
                && input_id != &requested_input_id
            {
                let rekeyed = state
                    .rekey_turn_context(&requested_input_id, input_id.clone())
                    .await;
                if rekeyed {
                    context_input_id = input_id.clone();
                }
            }
        }

        // Terminal dedup: input already processed — idempotent success
        let Some(handle) = handle else {
            if let Some(state) = state.as_ref()
                && queued_context
            {
                let _ = state.discard_turn_context(&context_input_id).await;
            }
            return Ok(());
        };

        let completion = handle.wait().await;
        if let Some(state) = state.as_ref()
            && queued_context
        {
            let _ = state.discard_turn_context(&context_input_id).await;
        }

        runtime_completion_to_mob_result(session_id, completion)
    }

    async fn admit_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<TurnEventTx>,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self.runtime_session_state(session_id).await;
        let adapter_session_id = session_id.clone();
        let requested_input_id = input.id().clone();
        let mut context_input_id = requested_input_id.clone();

        let queued_context = if let Some(ref state) = state {
            state
                .enqueue_turn_context(requested_input_id.clone(), event_tx)
                .await
        } else {
            false
        };

        let (outcome, handle) = match adapter
            .accept_input_with_completion(&adapter_session_id, input)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let Some(state) = state.as_ref()
                    && queued_context
                {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                return Err(MobError::Internal(err.to_string()));
            }
        };

        if let Some(state) = state.as_ref()
            && queued_context
        {
            let canonical_input_id = match &outcome {
                meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    Some(existing_id)
                }
                _ => None,
            };
            if let Some(input_id) = canonical_input_id
                && input_id != &requested_input_id
            {
                let rekeyed = state
                    .rekey_turn_context(&requested_input_id, input_id.clone())
                    .await;
                if rekeyed {
                    context_input_id = input_id.clone();
                }
            }
        }

        let Some(handle) = handle else {
            if let Some(state) = state.as_ref()
                && queued_context
            {
                let _ = state.discard_turn_context(&context_input_id).await;
            }
            return Ok(());
        };

        if let Some(state) = state
            && queued_context
        {
            tokio::spawn(async move {
                let _ = handle.wait().await;
                let _ = state.discard_turn_context(&context_input_id).await;
            });
        }

        Ok(())
    }
}

#[cfg(feature = "runtime-adapter")]
fn runtime_completion_to_mob_result(
    session_id: &SessionId,
    completion: meerkat_runtime::completion::CompletionOutcome,
) -> Result<(), MobError> {
    match completion {
        meerkat_runtime::completion::CompletionOutcome::Completed(_) => Ok(()),
        meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Ok(()),
        meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
            Err(MobError::CallbackPending {
                session_id: session_id.clone(),
                tool_name,
                args,
            })
        }
        meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
            Err(MobError::Internal(format!("turn abandoned: {reason}")))
        }
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
            Err(MobError::Internal(format!("runtime terminated: {reason}")))
        }
    }
}

fn session_turn_error_to_mob_error(bridge_session_id: &SessionId, error: SessionError) -> MobError {
    match error {
        SessionError::Agent(meerkat_core::error::AgentError::CallbackPending {
            tool_name,
            args,
        }) => MobError::CallbackPending {
            session_id: bridge_session_id.clone(),
            tool_name,
            args,
        },
        other => other.into(),
    }
}

#[cfg(feature = "runtime-adapter")]
struct RuntimeSessionState {
    // Transport-only owner context keyed by canonical runtime input identity.
    // Never used as lifecycle/ordering truth; ordering is runtime-owned via
    // contributing_input_ids and input lifecycle state.
    queued_turns: Mutex<RuntimeSessionQueue>,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Default)]
struct RuntimeSessionQueue {
    // Event delivery transport handles for runtime-backed turn dispatch.
    entries: HashMap<InputId, QueuedTurnContext>,
}

#[cfg(feature = "runtime-adapter")]
struct QueuedTurnContext {
    event_tx: TurnEventTx,
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeSessionState {
    async fn enqueue_turn_context(&self, input_id: InputId, event_tx: Option<TurnEventTx>) -> bool {
        let Some(event_tx) = event_tx else {
            return false;
        };
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns
            .entries
            .insert(input_id, QueuedTurnContext { event_tx });
        true
    }

    async fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[InputId],
    ) -> Option<QueuedTurnContext> {
        let mut queued_turns = self.queued_turns.lock().await;
        let mut selected: Option<QueuedTurnContext> = None;
        for input_id in contributing_input_ids {
            if let Some(context) = queued_turns.entries.remove(input_id) {
                // A runtime primitive may contribute multiple input IDs
                // (staged/apply-boundary cases). Drain every matching key so the
                // bridge map never accumulates stale side entries; prefer the
                // most-recent matching contributor in canonical order.
                selected = Some(context);
            }
        }
        selected
    }

    async fn rekey_turn_context(&self, from_input_id: &InputId, to_input_id: InputId) -> bool {
        if from_input_id == &to_input_id {
            return true;
        }
        let mut queued_turns = self.queued_turns.lock().await;
        if queued_turns.entries.contains_key(&to_input_id) {
            // Keep the canonical destination binding and drop the source alias.
            queued_turns.entries.remove(from_input_id);
            return true;
        }
        let Some(context) = queued_turns.entries.remove(from_input_id) else {
            return false;
        };
        queued_turns.entries.insert(to_input_id, context);
        true
    }

    async fn discard_turn_context(&self, input_id: &InputId) -> bool {
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns.entries.remove(input_id).is_some()
    }

    async fn clear_queued_turns(&self) {
        self.queued_turns.lock().await.entries.clear();
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        MultiBackendProvisioner, runtime_completion_to_mob_result, session_turn_error_to_mob_error,
    };
    use crate::error::MobError;
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use serde_json::json;

    #[test]
    fn runtime_callback_pending_maps_to_typed_mob_error() {
        let session_id = SessionId::new();
        let err = runtime_completion_to_mob_result(
            &session_id,
            meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            },
        )
        .expect_err("callback pending should remain resumable mob state");

        match err {
            MobError::CallbackPending {
                session_id: actual_session_id,
                tool_name,
                args,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, json!({ "value": "browser" }));
            }
            other => panic!("expected callback-pending mob error, got {other:?}"),
        }
    }

    #[test]
    fn direct_session_callback_pending_maps_to_typed_mob_error() {
        let session_id = SessionId::new();
        let err = session_turn_error_to_mob_error(
            &session_id,
            SessionError::Agent(meerkat_core::error::AgentError::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            }),
        );

        match err {
            MobError::CallbackPending {
                session_id: actual_session_id,
                tool_name,
                args,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, json!({ "value": "browser" }));
            }
            other => panic!("expected callback-pending mob error, got {other:?}"),
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn validated_external_peer_spec_preserves_the_validated_peer_name() {
        // Post-#24 `PeerId` is a typed UUID; `PeerId::parse` rejects the
        // legacy `ed25519:<alias>` shape that this test previously pinned.
        // The contract being asserted here is preservation of the peer
        // name + address through validation, so we feed a round-trippable
        // UUID string and round-trip through `PeerId::to_string()` on the
        // way out.
        let peer_id = meerkat_core::comms::PeerId::new();
        let peer_id_str = peer_id.to_string();
        let spec = MultiBackendProvisioner::validated_external_peer_spec(
            "mob/worker/member-1",
            &peer_id_str,
            "tcp://example.invalid/member-1",
            None,
        )
        .expect("external peer spec should validate");

        assert_eq!(spec.name.as_str(), "mob/worker/member-1");
        assert_eq!(spec.peer_id.to_string(), peer_id_str);
        assert_eq!(spec.address.to_string(), "tcp://example.invalid/member-1");
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn session_owned_eager_member_create_gets_runtime_execution_kind_stamp() {
        let mut req = meerkat_core::service::CreateSessionRequest {
            model: "gpt-5.4".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                initial_turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
                        ..Default::default()
                    },
                ),
                ..Default::default()
            }),
            labels: None,
        };

        super::stamp_eager_session_owned_initial_turn_metadata(&mut req);

        let metadata = req
            .build
            .as_ref()
            .and_then(|build| build.initial_turn_metadata.as_ref())
            .expect("eager runtime-owned member create should be stamped");
        assert_eq!(
            metadata.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
        assert_eq!(
            metadata.handling_mode,
            Some(meerkat_core::types::HandlingMode::Queue)
        );
    }
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<MeerkatMachine>,
    bridge_session_id: SessionId,
    state: Arc<RuntimeSessionState>,
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

#[cfg(feature = "runtime-adapter")]
impl MobSessionRuntimeExecutor {
    fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Arc<MeerkatMachine>,
        bridge_session_id: SessionId,
        state: Arc<RuntimeSessionState>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            bridge_session_id,
            state,
            runtime_sessions,
        }
    }
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionRuntimeControlHandle {
    session_service: Arc<dyn MobSessionService>,
    bridge_session_id: SessionId,
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorControl for MobSessionRuntimeControlHandle {
    async fn control(&self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        if command.should_interrupt_current_run() {
            return self
                .session_service
                .interrupt(&self.bridge_session_id)
                .await
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        if matches!(command, RunControlCommand::InterruptYielding) {
            return self
                .session_service
                .cancel_after_boundary(&self.bridge_session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } | SessionError::Unsupported(_) => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        Ok(())
    }
}

#[cfg(feature = "runtime-adapter")]
fn render_runtime_context_append_text(content: &CoreRenderable) -> String {
    match content {
        CoreRenderable::Text { text } => text.clone(),
        CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
        CoreRenderable::Json { value } => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        CoreRenderable::Reference { uri, label } => match label {
            Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
            _ => format!("[Reference] {uri}"),
        },
        _ => String::new(),
    }
}

#[cfg(feature = "runtime-adapter")]
fn pending_system_context_appends_for_runtime_executor(
    appends: &[meerkat_core::lifecycle::run_primitive::ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            text: render_runtime_context_append_text(&append.content),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
        })
        .collect()
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutor for MobSessionRuntimeExecutor {
    fn control_handle(&self) -> Option<Arc<dyn CoreExecutorControl>> {
        Some(Arc::new(MobSessionRuntimeControlHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: CoreRunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        // Context-only staged primitives may land directly as runtime
        // system-context appends, but terminal peer responses carry a typed
        // apply intent that requires a requester reaction turn.
        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                unreachable!("context-only apply without turn only matches staged primitives");
            };
            return self
                .session_service
                .apply_runtime_context_appends_with_boundary(
                    &self.bridge_session_id,
                    run_id,
                    pending_system_context_appends_for_runtime_executor(&staged.context_appends),
                    staged.boundary,
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()));
        }

        let contributing_input_ids = primitive.contributing_input_ids().to_vec();
        let pre_turn_context_appends = match &primitive {
            RunPrimitive::StagedInput(staged)
                if primitive.is_peer_response_terminal_context_and_run() =>
            {
                pending_system_context_appends_for_runtime_executor(&staged.context_appends)
            }
            _ => Vec::new(),
        };
        let queued_context = self
            .state
            .take_turn_context_for_inputs(&contributing_input_ids)
            .await;
        let req = StartTurnRequest {
            prompt: primitive.extract_content_input(),
            system_prompt: None,
            render_metadata: primitive
                .turn_metadata()
                .and_then(|meta| meta.render_metadata.clone()),
            handling_mode: primitive
                .turn_metadata()
                .and_then(|meta| meta.handling_mode)
                .unwrap_or(meerkat_core::types::HandlingMode::Queue),
            event_tx: queued_context.map(|context| context.event_tx),
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            pre_turn_context_appends,
            turn_metadata: primitive.turn_metadata().cloned(),
        };

        self.session_service
            .apply_runtime_turn(
                &self.bridge_session_id,
                run_id,
                req,
                match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                contributing_input_ids,
            )
            .await
            .map_err(|err| CoreExecutorError::apply_failed_runtime_turn(err.to_string()))
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            command if command.should_interrupt_current_run() => self
                .session_service
                .interrupt(&self.bridge_session_id)
                .await
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string())),
            RunControlCommand::InterruptYielding => self
                .session_service
                .cancel_after_boundary(&self.bridge_session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } | SessionError::Unsupported(_) => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string())),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                tracing::debug!(
                    bridge_session_id = %self.bridge_session_id,
                    "mob runtime executor received StopRuntimeExecutor; discarding live session"
                );
                let discard_result = self
                    .session_service
                    .discard_live_session(&self.bridge_session_id)
                    .await;
                self.runtime_adapter
                    .unregister_session(&self.bridge_session_id)
                    .await;
                let removed = {
                    let mut runtime_sessions = self.runtime_sessions.write().await;
                    let should_remove = runtime_sessions
                        .get(&self.bridge_session_id)
                        .is_some_and(|state| Arc::ptr_eq(state, &self.state));
                    if should_remove {
                        runtime_sessions.remove(&self.bridge_session_id)
                    } else {
                        None
                    }
                };
                if let Some(state) = removed {
                    state.clear_queued_turns().await;
                } else {
                    self.state.clear_queued_turns().await;
                }
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::control_failed_runtime(err.to_string())),
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for SessionBackend {
    async fn provision_member(
        &self,
        mut req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        tracing::debug!(
            binding = ?req.binding,
            peer_name = %req.peer_name,
            "SessionBackend::provision_member start"
        );
        let admitted_bridge_session_id = req
            .create_session
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone());
        // Pre-register with the runtime adapter so the factory receives
        // epoch-local bindings instead of creating a competing registry.
        let pre_registered_bridge_session_id = if let Some(adapter) = &self.runtime_adapter {
            if req.create_session.build.is_none() {
                req.create_session.build =
                    Some(meerkat_core::service::SessionBuildOptions::default());
            }
            let member_bridge_session_id = req
                .create_session
                .build
                .as_ref()
                .and_then(|b| b.resume_session.as_ref())
                .map(|s| s.id().clone())
                .unwrap_or_else(|| {
                    let id = SessionId::new();
                    let session = meerkat_core::session::Session::with_id(id.clone());
                    if let Some(ref mut build) = req.create_session.build {
                        build.resume_session = Some(session);
                    }
                    id
                });
            let bindings = adapter
                .prepare_local_session_bindings(member_bridge_session_id.clone())
                .await
                .map_err(|e| {
                    MobError::Internal(format!("prepare_local_session_bindings failed: {e}"))
                })?;
            if let Some(ref mut build) = req.create_session.build {
                build.runtime_build_mode =
                    meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);
            }
            stamp_eager_session_owned_initial_turn_metadata(&mut req.create_session);
            Some(member_bridge_session_id)
        } else {
            None
        };
        let created = match self
            .session_service
            .create_session(req.create_session)
            .await
        {
            Ok(created) => created,
            Err(e) => {
                // Rollback: unregister the pre-registered session on failure
                if let (Some(adapter), Some(pre_id)) =
                    (&self.runtime_adapter, &pre_registered_bridge_session_id)
                {
                    adapter.unregister_session(pre_id).await;
                }
                return Err(e.into());
            }
        };
        let created_bridge_session_id = created.session_id.clone();
        if let Some(admitted_bridge_session_id) = admitted_bridge_session_id.as_ref()
            && admitted_bridge_session_id != &created_bridge_session_id
        {
            if let Some(adapter) = &self.runtime_adapter {
                adapter.unregister_session(admitted_bridge_session_id).await;
                adapter.unregister_session(&created_bridge_session_id).await;
            }
            self.remove_runtime_session_state(admitted_bridge_session_id)
                .await;
            self.remove_runtime_session_state(&created_bridge_session_id)
                .await;
            if let Err(error) = self
                .session_service
                .archive(&created_bridge_session_id)
                .await
                && !matches!(error, SessionError::NotFound { .. })
            {
                return Err(MobError::Internal(format!(
                    "session service returned bridge session '{created_bridge_session_id}' for admitted mob spawn session '{admitted_bridge_session_id}', and cleanup archive failed: {error}"
                )));
            }
            return Err(MobError::Internal(format!(
                "session service returned bridge session '{created_bridge_session_id}' for admitted mob spawn session '{admitted_bridge_session_id}'"
            )));
        }
        // If no admission id was supplied, clean up stale local pre-registration
        // defensively. Normal mob spawn paths now admit a concrete session id
        // before provisioning starts.
        if let (Some(adapter), Some(pre_id)) =
            (&self.runtime_adapter, &pre_registered_bridge_session_id)
        {
            if *pre_id != created_bridge_session_id {
                tracing::debug!(
                    pre_registered = %pre_id,
                    actual_bridge_session_id = %created_bridge_session_id,
                    "mob provisioner: session service returned different ID; reconciling runtime registration"
                );
                adapter.unregister_session(pre_id).await;
            }
            let _ = self.runtime_session_state(&created_bridge_session_id).await;
        }
        if let (Some(owner_bridge_session_id), Some(registry)) =
            (req.owner_bridge_session_id, req.ops_registry)
        {
            self.ops_adapter.bind_session_registry(
                created_bridge_session_id.clone(),
                owner_bridge_session_id,
                registry,
            );
        }
        let operation_id = self
            .ops_adapter
            .mark_member_provisioned(&created_bridge_session_id, &req.peer_name)
            .await?;
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            "SessionBackend::provision_member created bridge session"
        );
        Ok(MemberSpawnReceipt {
            member_ref: MemberRef::from_bridge_session_id(created_bridge_session_id),
            operation_id,
        })
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        let bridge_session_id = Self::require_session(member_ref, "abort provision for")?;
        match self
            .ops_adapter
            .operation_status(&bridge_session_id, operation_id)
        {
            Some(OperationStatus::Provisioning) => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&bridge_session_id).await {
                        adapter.unregister_session(&bridge_session_id).await;
                    }
                    self.remove_runtime_session_state(&bridge_session_id).await;
                }
                match self.session_service.archive(&bridge_session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                self.ops_adapter
                    .abort_member_provision(
                        &bridge_session_id,
                        operation_id,
                        Some(reason.to_string()),
                    )
                    .await
            }
            Some(OperationStatus::Running | OperationStatus::Retiring) => {
                self.retire_member(member_ref).await
            }
            Some(
                OperationStatus::Completed
                | OperationStatus::Failed
                | OperationStatus::Aborted
                | OperationStatus::Cancelled
                | OperationStatus::Retired
                | OperationStatus::Terminated,
            ) => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&bridge_session_id).await {
                        adapter.unregister_session(&bridge_session_id).await;
                    }
                    self.remove_runtime_session_state(&bridge_session_id).await;
                }
                match self.session_service.archive(&bridge_session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some(OperationStatus::Absent) | None => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&bridge_session_id).await {
                        adapter.unregister_session(&bridge_session_id).await;
                    }
                    self.remove_runtime_session_state(&bridge_session_id).await;
                }
                match self.session_service.archive(&bridge_session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "retire")?;
        if let Some(adapter) = &self.runtime_adapter {
            if adapter.contains_session(&session_id).await {
                LocalMobRuntimeBridge::new(adapter.clone(), session_id.clone())
                    .retire_member()
                    .await
                    .map(|_| ())
                    .map_err(|err| MobError::Internal(err.to_string()))?;
                adapter.unregister_session(&session_id).await;
            }
            self.remove_runtime_session_state(&session_id).await;
        }
        self.session_service.archive(&session_id).await?;
        self.ops_adapter.mark_member_retired(member_ref).await?;
        Ok(())
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        if let Some(adapter) = &self.runtime_adapter {
            if adapter.contains_session(&session_id).await {
                return match LocalMobRuntimeBridge::new(adapter.clone(), session_id.clone())
                    .interrupt_member()
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        // Preserve bridge sidecar alignment when registration
                        // changed between contains_session and interrupt.
                        if !adapter.contains_session(&session_id).await {
                            self.remove_runtime_session_state(&session_id).await;
                        }
                        Err(MobError::Internal(format!(
                            "runtime-backed interrupt must resolve through MeerkatMachine for '{session_id}': {error}"
                        )))
                    }
                };
            }

            // Runtime-backed members must be interrupted through runtime
            // adapter registration truth, not direct session-service fallback.
            self.remove_runtime_session_state(&session_id).await;
            return Err(MobError::Internal(format!(
                "runtime-backed interrupt requested for unregistered runtime session '{session_id}'"
            )));
        }
        self.session_service.interrupt(&session_id).await?;
        Ok(())
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start turn")?;
        if self.runtime_adapter.is_some() {
            self.ops_adapter
                .report_member_progress(member_ref, "turn dispatched")
                .await?;
            let turn_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(req.handling_mode),
                keep_alive: None,
                skill_references: req.skill_references.clone(),
                flow_tool_overlay: req.flow_tool_overlay.clone(),
                render_metadata: req.render_metadata.clone(),
                ..Default::default()
            };
            let prompt = req.prompt.clone();
            let input = Input::Prompt(PromptInput {
                header: InputHeader {
                    id: meerkat_core::InputId::new(),
                    timestamp: chrono::Utc::now(),
                    source: InputOrigin::Operator,
                    durability: InputDurability::Durable,
                    visibility: InputVisibility::default(),
                    idempotency_key: None,
                    supersession_key: None,
                    correlation_id: None,
                },
                text: prompt.text_content(),
                blocks: if prompt.has_images() {
                    Some(prompt.into_blocks())
                } else {
                    None
                },
                turn_metadata: Some(turn_metadata),
            });
            return self
                .execute_runtime_input(&session_id, input, req.event_tx)
                .await;
        }

        self.session_service
            .start_turn(&session_id, req)
            .await
            .map(|_| ())
            .map_err(|error| session_turn_error_to_mob_error(&session_id, error))
    }

    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start flow step")?;
        if self.runtime_adapter.is_some() {
            let input = meerkat_runtime::mob_adapter::create_flow_step_input(
                step_id.as_str(),
                req.prompt.clone(),
                &run_id.to_string(),
                0,
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        handling_mode: Some(req.handling_mode),
                        keep_alive: None,
                        skill_references: req.skill_references.clone(),
                        flow_tool_overlay: req.flow_tool_overlay.clone(),
                        render_metadata: req.render_metadata.clone(),
                        ..Default::default()
                    },
                ),
            );
            return self
                .admit_runtime_input(&session_id, input, req.event_tx)
                .await;
        }

        self.start_turn(member_ref, req).await
    }

    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session_service
            .interaction_event_injector(bridge_session_id)
            .await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        let bridge_session_id = match member_ref.bridge_session_id() {
            Some(id) => id.clone(),
            None => return Ok(None),
        };
        match self.session_service.read(&bridge_session_id).await {
            Ok(view) => Ok(Some(view.state.is_active)),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => Ok(Some(false)),
            Err(error) => Err(error.into()),
        }
    }

    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let bridge_session_id = Self::require_session(member_ref, "ensure runtime session for")?;
        self.runtime_session_state(&bridge_session_id)
            .await
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "runtime adapter unavailable while ensuring session state for '{bridge_session_id}'"
                ))
            })?;
        Ok(())
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        let bridge_session_id = member_ref.bridge_session_id()?;
        self.session_service.comms_runtime(bridge_session_id).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let trusted_peer = Self::trusted_peer_spec(fallback_name, fallback_peer_id)?;
        self.ops_adapter
            .mark_member_peer_ready(member_ref, fallback_name, trusted_peer.clone())
            .await?;
        Ok(trusted_peer)
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        let bridge_session_id = member_ref.bridge_session_id()?;
        self.ops_adapter
            .active_operation_id_for_session(bridge_session_id)
            .await
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        let Some(bridge_session_id) = member_ref.bridge_session_id().cloned() else {
            return Err(MobError::Internal(
                "member has no session bridge for canonical ops binding".into(),
            ));
        };
        self.ops_adapter.bind_session_registry(
            bridge_session_id,
            owner_bridge_session_id,
            ops_registry,
        );
        Ok(())
    }

    async fn cancel_all_checkpointers(&self) {
        self.session_service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session_service.rearm_all_checkpointers().await;
    }
}

pub struct ExternalBackend {
    _session_service: Arc<dyn MobSessionService>,
}

impl ExternalBackend {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        _config: ExternalBackendConfig,
    ) -> Self {
        Self {
            _session_service: session_service,
        }
    }
}

fn is_valid_peer_name_component(component: &str) -> bool {
    if component.is_empty() {
        return false;
    }
    let mut chars = component.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_valid_external_peer_name(peer_name: &str) -> bool {
    let mut parts = peer_name.split('/');
    let Some(mob_id) = parts.next() else {
        return false;
    };
    let Some(profile) = parts.next() else {
        return false;
    };
    let Some(meerkat_id) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [mob_id, profile, meerkat_id]
        .iter()
        .all(|part| is_valid_peer_name_component(part))
}

#[cfg(feature = "runtime-adapter")]
pub struct MultiBackendProvisioner {
    session: SessionBackend,
    external: Option<ExternalBackend>,
    supervisor_bridge: Arc<super::MobSupervisorBridge>,
    binding_persistence: Option<ProvisionerBindingPersistence>,
}

#[cfg(feature = "runtime-adapter")]
struct ExternalBindingTarget {
    peer_name: String,
    peer_id: String,
    address: String,
    bootstrap_token: Option<super::bridge_protocol::BridgeBootstrapToken>,
    /// Ed25519 signing pubkey of the external process. When present,
    /// propagated into the supervisor's trust store so inbound
    /// signed-envelope replies admit past ingress `is_trusted` gating.
    pubkey: Option<[u8; 32]>,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
struct ProvisionerBindingPersistence {
    mob_id: crate::MobId,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    roster: Arc<RwLock<super::roster_authority::RosterAuthority>>,
}

#[cfg(feature = "runtime-adapter")]
impl MultiBackendProvisioner {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
        external: Option<ExternalBackendConfig>,
        supervisor_bridge: Arc<super::MobSupervisorBridge>,
    ) -> Self {
        let session = SessionBackend::new(session_service.clone(), runtime_adapter);
        let external = external.map(|cfg| ExternalBackend::new(session_service, cfg));
        Self {
            session,
            external,
            supervisor_bridge,
            binding_persistence: None,
        }
    }

    pub fn with_binding_persistence(
        mut self,
        mob_id: crate::MobId,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        roster: Arc<RwLock<super::roster_authority::RosterAuthority>>,
    ) -> Self {
        self.binding_persistence = Some(ProvisionerBindingPersistence {
            mob_id,
            runtime_metadata,
            roster,
        });
        self
    }

    async fn peer_only_spec(
        &self,
        member_ref: &MemberRef,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id: None,
                ..
            } => Self::peer_only_spec_from_parts(peer_id, address),
            _ => Err(MobError::Internal(
                "peer-only spec requested for non-peer-only member".to_string(),
            )),
        }
    }

    fn peer_only_spec_from_parts(
        peer_id: &str,
        address: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let peer_name = address
            .strip_prefix("inproc://")
            .map(|value| value.split('?').next().unwrap_or(value).to_string())
            .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}"));
        let pubkey = meerkat_comms::InprocRegistry::global()
            .get_by_name(&peer_name)
            .and_then(|(registry_pubkey, _)| {
                (registry_pubkey.to_peer_id().as_str() == peer_id)
                    .then(|| *registry_pubkey.as_bytes())
            });
        let result = match pubkey {
            Some(pubkey) => TrustedPeerDescriptor::unsigned_with_pubkey(
                peer_name,
                peer_id.to_string(),
                pubkey,
                address.to_string(),
            ),
            None => TrustedPeerDescriptor::test_only_unsigned(
                peer_name,
                peer_id.to_string(),
                address.to_string(),
            ),
        };
        result.map_err(|error| MobError::WiringError(format!("invalid peer-only spec: {error}")))
    }

    fn validated_external_peer_spec(
        peer_name: &str,
        peer_id: &str,
        address: &str,
        pubkey: Option<[u8; 32]>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let result = match pubkey {
            Some(pubkey) => TrustedPeerDescriptor::unsigned_with_pubkey(
                peer_name.to_string(),
                peer_id.to_string(),
                pubkey,
                address.to_string(),
            ),
            None => TrustedPeerDescriptor::test_only_unsigned(
                peer_name.to_string(),
                peer_id.to_string(),
                address.to_string(),
            ),
        };
        result.map_err(|error| {
            MobError::WiringError(format!(
                "invalid external peer spec for '{peer_name}': {error}"
            ))
        })
    }

    fn bridge_bootstrap_token_from_binding(
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> Result<super::bridge_protocol::BridgeBootstrapToken, MobError> {
        bootstrap_token
            .filter(|token| !token.is_empty())
            .map(super::bridge_protocol::BridgeBootstrapToken::new)
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "external runtime binding for '{address}' is missing typed bootstrap_token field"
                ))
            })
    }

    async fn bridge_supervisor_payload(
        &self,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        let spec = self.supervisor_bridge.supervisor_spec().await?;
        Ok(super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: spec.into(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
        })
    }

    fn bridge_rejection_reply(
        protocol_version: super::bridge_protocol::BridgeProtocolVersion,
        value: &serde_json::Value,
    ) -> Option<super::bridge_protocol::BridgeRejectionReply> {
        super::bridge_protocol::decode_bridge_rejection_reply(protocol_version, value)
    }

    fn bridge_rejection_error(rejection: super::bridge_protocol::BridgeRejectionReply) -> MobError {
        MobError::from(rejection)
    }

    async fn ensure_supervisor_authorized(
        &self,
        peer: &TrustedPeerDescriptor,
        binding: Option<(&str, &str, Option<&str>)>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        let protocol_version = payload.protocol_version;
        let command = super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(payload);
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, &command, Duration::from_secs(30))
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            if let Some(cause) = rejection.typed_cause()
                && super::bridge_fallback::should_fall_back_to_bind(cause)
                && let Some((peer_id, address, bootstrap_token)) = binding
            {
                let bind: super::bridge_protocol::BridgeBindResponse = self
                    .bind_peer_only_member(peer, peer_id, address, bootstrap_token)
                    .await?;
                let effective_bootstrap_token =
                    Self::bridge_bootstrap_token_from_binding(address, bootstrap_token)?;
                self.persist_rebound_binding(
                    peer_id,
                    Some(effective_bootstrap_token.clone()),
                    &bind,
                )
                .await?;
                return Self::peer_only_spec_from_parts(&bind.peer_id, &bind.address);
            }
            return Err(Self::bridge_rejection_error(rejection));
        }
        Ok(peer.clone())
    }

    async fn send_bridge_command_typed<R: DeserializeOwned>(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<R, MobError> {
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, command, timeout)
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(Self::bridge_rejection_error(rejection));
        }
        serde_json::from_value(value).map_err(|error| {
            MobError::Internal(format!("failed to decode bridge command response: {error}"))
        })
    }

    async fn bind_peer_only_member(
        &self,
        peer: &TrustedPeerDescriptor,
        peer_id: &str,
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
        let bootstrap_token = Self::bridge_bootstrap_token_from_binding(address, bootstrap_token)?;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        let command = super::bridge_protocol::BridgeCommand::BindMember(
            super::bridge_protocol::BridgeBindPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                expected_peer_id: peer_id.to_string(),
                expected_address: address.to_string(),
                bootstrap_token,
            },
        );
        self.send_bridge_command_typed(peer, &command, Duration::from_secs(30))
            .await
    }

    async fn persist_rebound_binding(
        &self,
        prior_peer_id: &str,
        bootstrap_token: Option<super::bridge_protocol::BridgeBootstrapToken>,
        bind: &super::bridge_protocol::BridgeBindResponse,
    ) -> Result<(), MobError> {
        let Some(persistence) = self.binding_persistence.as_ref() else {
            return Ok(());
        };
        let canonical_address = super::bridge_protocol::canonicalize_bridge_address(&bind.address);
        let updated_entries = persistence
            .roster
            .write()
            .await
            .replace_backend_peer_binding_by_peer_id(
                prior_peer_id,
                &bind.peer_id,
                &canonical_address,
                bootstrap_token.clone(),
            );
        for (identity, generation) in updated_entries {
            persistence
                .runtime_metadata
                .upsert_external_binding_overlay(
                    &persistence.mob_id,
                    &crate::store::ExternalBindingOverlayRecord {
                        agent_identity: identity,
                        generation,
                        normalized_member_ref: Some(MemberRef::BackendPeer {
                            peer_id: bind.peer_id.clone(),
                            address: canonical_address.clone(),
                            bootstrap_token: None,
                            session_id: None,
                        }),
                        bootstrap_token: bootstrap_token.clone(),
                        status: crate::store::ExternalBindingOverlayStatus::Normalized,
                        updated_at: chrono::Utc::now(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn external_member_ref(
        &self,
        _create_session: CreateSessionRequest,
        owner_bridge_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
        target: ExternalBindingTarget,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let ExternalBindingTarget {
            peer_name,
            peer_id: real_peer_id,
            address: real_address,
            bootstrap_token,
            pubkey,
        } = target;
        let effective_bootstrap_token = Self::bridge_bootstrap_token_from_binding(
            &real_address,
            bootstrap_token
                .as_ref()
                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
        )?;
        if !is_valid_external_peer_name(&peer_name) {
            return Err(MobError::WiringError(format!(
                "invalid external peer name '{peer_name}': expected '<mob>/<profile>/<meerkat>' using identifier-safe segments"
            )));
        }
        tracing::debug!(
            peer_name = %peer_name,
            "ExternalBackend::external_member_ref start"
        );
        let _external = self
            .external
            .as_ref()
            .ok_or_else(|| MobError::WiringError("external backend is not configured".into()))?;
        tracing::debug!(
            peer_id = %real_peer_id,
            address = %real_address,
            "ExternalBackend::external_member_ref success (real identity)"
        );
        let peer =
            Self::validated_external_peer_spec(&peer_name, &real_peer_id, &real_address, pubkey)?;
        let bind_response = self
            .bind_peer_only_member(
                &peer,
                &real_peer_id,
                &real_address,
                Some(effective_bootstrap_token.as_str()),
            )
            .await?;
        let member_ref = MemberRef::BackendPeer {
            peer_id: bind_response.peer_id,
            address: super::bridge_protocol::canonicalize_bridge_address(&bind_response.address),
            bootstrap_token: Some(effective_bootstrap_token),
            session_id: None,
        };
        if let (Some(owner_bridge_session_id), Some(registry)) =
            (owner_bridge_session_id, ops_registry)
        {
            self.session.ops_adapter.bind_member_registry(
                &member_ref,
                owner_bridge_session_id,
                registry,
                peer_name.clone(),
            )?;
        }
        let operation_id = self
            .session
            .ops_adapter
            .mark_member_provisioned_for_member(&member_ref, &peer_name)
            .await?;
        Ok(MemberSpawnReceipt {
            member_ref,
            operation_id,
        })
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for MultiBackendProvisioner {
    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        match req.binding {
            RuntimeBinding::Session => {
                self.session
                    .provision_member(ProvisionMemberRequest {
                        create_session: req.create_session,
                        binding: RuntimeBinding::Session,
                        peer_name: req.peer_name,
                        owner_bridge_session_id: req.owner_bridge_session_id,
                        ops_registry: req.ops_registry,
                    })
                    .await
            }
            RuntimeBinding::External {
                peer_id,
                address,
                bootstrap_token,
                pubkey,
            } => {
                self.external_member_ref(
                    req.create_session,
                    req.owner_bridge_session_id,
                    req.ops_registry,
                    ExternalBindingTarget {
                        peer_name: req.peer_name,
                        peer_id,
                        address,
                        bootstrap_token,
                        pubkey,
                    },
                )
                .await
            }
        }
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => {
                self.session
                    .ops_adapter
                    .abort_member_provision_for_member(
                        member_ref,
                        operation_id,
                        Some(reason.to_string()),
                    )
                    .await
            }
            _ => {
                self.session
                    .abort_member_provision(member_ref, operation_id, reason)
                    .await
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: None,
                ..
            } => {
                let peer = self.peer_only_spec(member_ref).await?;
                let peer = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                        )),
                    )
                    .await?;
                let payload = self.bridge_supervisor_payload().await?;
                let command = super::bridge_protocol::BridgeCommand::RetireMember(payload);
                let _retire: super::bridge_protocol::BridgeRetireResponse = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(10))
                    .await?;
                self.session
                    .ops_adapter
                    .mark_member_retired(member_ref)
                    .await
            }
            _ => self.session.retire_member(member_ref).await,
        }
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: None,
                ..
            } => {
                let peer = self.peer_only_spec(member_ref).await?;
                let peer = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                        )),
                    )
                    .await?;
                let payload = self.bridge_supervisor_payload().await?;
                let command = super::bridge_protocol::BridgeCommand::InterruptMember(payload);
                let _ack: super::bridge_protocol::BridgeAck = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await?;
                Ok(())
            }
            _ => self.session.interrupt_member(member_ref).await,
        }
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: None,
                ..
            } => {
                if req.event_tx.is_some() {
                    return Err(MobError::UnsupportedForMode {
                        mode: crate::MobRuntimeMode::TurnDriven,
                        reason: "tracked turn event streams are not supported for peer-only members in phase 1".to_string(),
                    });
                }
                let peer = self.peer_only_spec(member_ref).await?;
                let peer = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                        )),
                    )
                    .await?;
                let authority = self.supervisor_bridge.authority().await;
                let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
                let command = super::bridge_protocol::BridgeCommand::DeliverMemberInput(
                    super::bridge_protocol::BridgeDeliveryPayload {
                        supervisor: sup_spec.into(),
                        epoch: authority.epoch,
                        protocol_version: authority.protocol_version,
                        input_id: Uuid::now_v7().to_string(),
                        content: req.prompt.clone(),
                        handling_mode: req.handling_mode,
                    },
                );
                let response: super::bridge_protocol::BridgeDeliveryResponse = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await?;
                match response.outcome {
                    super::bridge_protocol::BridgeDeliveryOutcome::Accepted
                    | super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { .. } => {}
                    super::bridge_protocol::BridgeDeliveryOutcome::Rejected { cause, reason } => {
                        return Err(MobError::BridgeDeliveryRejected { cause, reason });
                    }
                }
                self.session
                    .ops_adapter
                    .report_member_progress(member_ref, "turn dispatched")
                    .await?;
                Ok(())
            }
            _ => self.session.start_turn(member_ref, req).await,
        }
    }

    async fn reconcile_peer_only_trust(
        &self,
        member_ref: &MemberRef,
        desired_specs: &[TrustedPeerDescriptor],
    ) -> Result<(), MobError> {
        let MemberRef::BackendPeer {
            peer_id,
            address,
            bootstrap_token,
            session_id: None,
            ..
        } = member_ref
        else {
            return Ok(());
        };

        let peer = self.peer_only_spec(member_ref).await?;
        let peer = self
            .ensure_supervisor_authorized(
                &peer,
                Some((
                    peer_id.as_str(),
                    address.as_str(),
                    bootstrap_token
                        .as_ref()
                        .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                )),
            )
            .await?;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        for spec in desired_specs {
            let command = super::bridge_protocol::BridgeCommand::WireMember(
                super::bridge_protocol::BridgePeerWiringPayload {
                    supervisor: sup_spec.clone().into(),
                    epoch: authority.epoch,
                    protocol_version: authority.protocol_version,
                    peer_spec: spec.clone().into(),
                },
            );
            let _ack: super::bridge_protocol::BridgeAck = self
                .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                .await?;
        }
        Ok(())
    }

    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session
            .interaction_event_injector(bridge_session_id)
            .await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Ok(None),
            _ => self.session.is_member_active(member_ref).await,
        }
    }

    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Ok(()),
            _ => self.session.ensure_runtime_session_state(member_ref).await,
        }
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => None,
            _ => self.session.comms_runtime(member_ref).await,
        }
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.session
                    .trusted_peer_spec(member_ref, fallback_name, fallback_peer_id)
                    .await
            }
            MemberRef::BackendPeer { session_id, .. } => {
                if let Some(session_id) = session_id {
                    // External members keep a local bridge session for lifecycle
                    // transport (notifications, kickoff events). The trust spec
                    // uses the bridge session's comms identity — NOT the real
                    // external peer_id — because the bridge signs lifecycle
                    // messages with its own keypair. The caller-provided
                    // `fallback_peer_id` is the bridge's `comms.public_key()`,
                    // set by `do_wire`'s key resolution path.
                    return self
                        .session
                        .trusted_peer_spec(
                            &MemberRef::Session {
                                session_id: session_id.clone(),
                            },
                            fallback_name,
                            fallback_peer_id,
                        )
                        .await;
                }
                // No bridge — use the real BackendPeer identity directly.
                let mut spec = self.peer_only_spec(member_ref).await?;
                spec.name = meerkat_core::comms::PeerName::new(fallback_name.to_string()).map_err(
                    |error| MobError::WiringError(format!("invalid peer name: {error}")),
                )?;
                Ok(spec)
            }
        }
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => {
                self.session
                    .ops_adapter
                    .active_operation_id_for_member(member_ref)
                    .await
            }
            _ => {
                let bridge_session_id = member_ref.bridge_session_id()?;
                self.session
                    .ops_adapter
                    .active_operation_id_for_session(bridge_session_id)
                    .await
            }
        }
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id: None,
                ..
            } => {
                let display_name = format!("mob_member/backend_peer/{peer_id}@{address}");
                self.session.ops_adapter.bind_member_registry(
                    member_ref,
                    owner_bridge_session_id,
                    ops_registry,
                    display_name.clone(),
                )?;
                let _ = self
                    .session
                    .ops_adapter
                    .mark_member_provisioned_for_member(member_ref, &display_name)
                    .await?;
                Ok(())
            }
            _ => {
                self.session
                    .bind_member_owner_context(member_ref, owner_bridge_session_id, ops_registry)
                    .await
            }
        }
    }

    async fn cancel_all_checkpointers(&self) {
        self.session.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session.rearm_all_checkpointers().await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod bridge_rejection_tests {
    use super::MultiBackendProvisioner;
    use crate::MobError;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        decode_legacy_v1_raw_string_rejection,
    };
    use serde_json::json;

    fn known_bridge_rejection_causes() -> &'static [(BridgeRejectionCause, &'static str)] {
        &[
            (BridgeRejectionCause::NotBound, "not_bound"),
            (BridgeRejectionCause::StaleSupervisor, "stale_supervisor"),
            (BridgeRejectionCause::SenderMismatch, "sender_mismatch"),
            (BridgeRejectionCause::AlreadyBound, "already_bound"),
            (
                BridgeRejectionCause::InvalidBootstrapToken,
                "invalid_bootstrap_token",
            ),
            (
                BridgeRejectionCause::UnsupportedProtocolVersion,
                "unsupported_protocol_version",
            ),
            (
                BridgeRejectionCause::InvalidSupervisorSpec,
                "invalid_supervisor_spec",
            ),
            (BridgeRejectionCause::InvalidPeerSpec, "invalid_peer_spec"),
            (BridgeRejectionCause::AddressMismatch, "address_mismatch"),
            (BridgeRejectionCause::Unsupported, "unsupported"),
            (BridgeRejectionCause::Internal, "internal"),
        ]
    }

    #[test]
    fn provisioner_decodes_typed_protocol_v2_bridge_rejection() {
        let value = json!({
            "result": "rejected",
            "cause": "not_bound",
            "reason": "bind required",
        });

        let rejection = MultiBackendProvisioner::bridge_rejection_reply(
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            &value,
        )
        .expect("typed rejection should decode");

        assert_eq!(
            rejection.typed_cause(),
            Some(BridgeRejectionCause::NotBound)
        );
        assert_eq!(rejection.reason(), "bind required");
    }

    #[test]
    fn provisioner_does_not_promote_raw_string_as_protocol_v2_rejection() {
        let value = json!("legacy rejection");

        assert!(
            MultiBackendProvisioner::bridge_rejection_reply(
                SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                &value,
            )
            .is_none()
        );
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");
        assert_eq!(legacy.typed_cause(), None);
        assert!(legacy.is_legacy_v1_raw_string());
    }

    #[test]
    fn provisioner_bridge_rejection_error_preserves_each_known_typed_cause() {
        for (cause, wire_name) in known_bridge_rejection_causes() {
            let reason = format!("typed bridge rejection: {wire_name}");
            let value = json!({
                "result": "rejected",
                "cause": wire_name,
                "reason": reason,
            });
            let rejection = MultiBackendProvisioner::bridge_rejection_reply(
                SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                &value,
            )
            .expect("typed rejection should decode");

            let error = MultiBackendProvisioner::bridge_rejection_error(rejection);

            match error {
                MobError::BridgeCommandRejected {
                    cause: actual,
                    reason: actual_reason,
                } => {
                    assert_eq!(actual, *cause);
                    assert_eq!(actual_reason, reason);
                }
                other => panic!("expected typed bridge rejection error, got {other:?}"),
            }
        }
    }

    #[test]
    fn provisioner_legacy_bridge_rejection_error_stays_untyped() {
        let value = json!("legacy rejection");
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");

        let error = MultiBackendProvisioner::bridge_rejection_error(legacy);

        assert!(matches!(
            error,
            MobError::WiringError(reason) if reason == "legacy rejection"
        ));
    }
}
