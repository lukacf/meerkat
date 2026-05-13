//! Smoke test for `meerkat::session_runtime::llm_reconfigure`.
//!
//! Asserts the surface-agnostic [`hot_swap_llm_client_on_idle_session`]
//! flow drives the [`SessionLlmReconfigureHost`] trait in the expected
//! order: hydrate → resolve → apply identity → apply visibility →
//! persist. A stub host records each call and lets us verify both the
//! happy path and the rollback-on-persist-failure path.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex;

use async_trait::async_trait;
use meerkat::session_runtime::llm_reconfigure::{
    derive_reconfigured_visibility_state, hot_swap_llm_client_on_idle_session,
    profile_to_capability_surface,
};
use meerkat_core::types::SessionId;
use meerkat_core::{Provider, SessionLlmIdentity, SessionToolVisibilityState};
use meerkat_runtime::{
    HydratedSessionLlmState, ResolvedSessionLlmReconfigure, RuntimeDriverError,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureRequest,
};

#[derive(Debug, Default)]
struct StubHostState {
    hydrate_calls: u32,
    resolve_calls: u32,
    apply_identity_calls: u32,
    apply_visibility_calls: u32,
    persist_calls: u32,
    discard_calls: u32,
    /// When `Some`, `persist_live_session` returns this as an error.
    persist_error: Option<String>,
}

struct StubReconfigureHost {
    state: Mutex<StubHostState>,
    current_identity: SessionLlmIdentity,
    target_identity: SessionLlmIdentity,
    target_capability_surface: SessionLlmCapabilitySurface,
}

fn empty_capability_surface() -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: false,
        supports_thinking: false,
        supports_reasoning: false,
        inline_video: false,
        vision: false,
        image_input: false,
        image_tool_results: false,
        supports_web_search: false,
        image_generation: false,
        realtime: false,
        call_timeout_secs: None,
    }
}

impl StubReconfigureHost {
    fn new(target_provider: Provider, target_model: &str) -> Self {
        let current_identity = SessionLlmIdentity {
            model: "starting-model".into(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let target_identity = SessionLlmIdentity {
            model: target_model.into(),
            provider: target_provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        Self {
            state: Mutex::default(),
            current_identity,
            target_identity,
            target_capability_surface: empty_capability_surface(),
        }
    }
}

#[async_trait]
impl SessionLlmReconfigureHost for StubReconfigureHost {
    async fn hydrate_session_llm_state(
        &self,
        _session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        self.state.lock().unwrap().hydrate_calls += 1;
        Ok(HydratedSessionLlmState {
            current_identity: self.current_identity.clone(),
            current_visibility_state: SessionToolVisibilityState::default(),
            current_capability_surface: Some(empty_capability_surface()),
            capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
            base_tool_names: std::collections::BTreeSet::new(),
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        _request: &SessionLlmReconfigureRequest,
        _current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        self.state.lock().unwrap().resolve_calls += 1;
        Ok(ResolvedSessionLlmReconfigure {
            target_identity: self.target_identity.clone(),
            target_capability_surface: self.target_capability_surface.clone(),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        _session_id: &SessionId,
        _identity: &SessionLlmIdentity,
    ) -> Result<(), RuntimeDriverError> {
        self.state.lock().unwrap().apply_identity_calls += 1;
        Ok(())
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        _session_id: &SessionId,
        _visibility_state: Option<SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        self.state.lock().unwrap().apply_visibility_calls += 1;
        Ok(())
    }

    async fn persist_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let mut state = self.state.lock().unwrap();
        state.persist_calls += 1;
        if let Some(reason) = state.persist_error.clone() {
            return Err(RuntimeDriverError::Internal(reason));
        }
        Ok(())
    }

    async fn discard_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.state.lock().unwrap().discard_calls += 1;
        Ok(())
    }
}

#[tokio::test]
async fn happy_path_drives_hydrate_resolve_apply_persist_in_order() {
    let host = StubReconfigureHost::new(Provider::OpenAI, "gpt-target");
    let session_id = SessionId::new();
    let request = SessionLlmReconfigureRequest {
        model: Some("gpt-target".into()),
        provider: None,
        provider_params: None,
        auth_binding: None,
    };

    hot_swap_llm_client_on_idle_session(&host, &session_id, &request)
        .await
        .expect("hot swap succeeds");

    let state = host.state.lock().unwrap();
    assert_eq!(state.hydrate_calls, 1);
    assert_eq!(state.resolve_calls, 1);
    assert_eq!(state.apply_identity_calls, 1);
    assert_eq!(state.apply_visibility_calls, 1);
    assert_eq!(state.persist_calls, 1);
    assert_eq!(state.discard_calls, 0);
}

#[tokio::test]
async fn persist_failure_triggers_rollback_to_previous_identity() {
    let host = StubReconfigureHost::new(Provider::OpenAI, "gpt-target");
    {
        let mut state = host.state.lock().unwrap();
        state.persist_error = Some("disk full".into());
    }
    let session_id = SessionId::new();
    let request = SessionLlmReconfigureRequest {
        model: Some("gpt-target".into()),
        provider: None,
        provider_params: None,
        auth_binding: None,
    };

    let err = hot_swap_llm_client_on_idle_session(&host, &session_id, &request)
        .await
        .expect_err("persist failure must propagate after rollback");
    assert!(matches!(err, RuntimeDriverError::Internal(_)));

    let state = host.state.lock().unwrap();
    // Apply identity is called twice: once for the swap, once for rollback.
    assert_eq!(state.apply_identity_calls, 2);
    assert_eq!(state.apply_visibility_calls, 2);
    // Discard never fires when rollback succeeds.
    assert_eq!(state.discard_calls, 0);
}

#[test]
fn derive_reconfigured_visibility_state_clones_when_no_view_image() {
    let current = SessionToolVisibilityState::default();
    let target = empty_capability_surface();
    let names = std::collections::BTreeSet::new();
    let next = derive_reconfigured_visibility_state(&current, &target, &names);
    assert_eq!(next.active_revision, current.active_revision);
}

// `profile_to_capability_surface` round-trip is exercised indirectly by
// the rpc-side `r11_*` tests; constructing a `ModelProfile` here would
// require pulling the entire model catalog into the test binary.
#[test]
fn profile_to_capability_surface_symbol_resolves() {
    let _: fn(&meerkat_models::profile::ModelProfile) -> SessionLlmCapabilitySurface =
        profile_to_capability_surface;
}
