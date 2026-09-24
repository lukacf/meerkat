//! Classification contract for the committed handoff-log read.
//!
//! The pre-dequeue seam reads this log on EVERY runtime-loop lap, before any
//! input is served. That makes its error classification load-bearing in a way
//! that is easy to get wrong in both directions:
//!
//! * too strict — an ordinary "no live actor yet" is reported as a read
//!   failure, the loop fails closed, and every session dies before reaching a
//!   provider (the `session/create` regression these tests pin);
//! * too loose — every failure is flattened into "nothing owed", and a
//!   committed handoff is silently dropped forever.
//!
//! These tests pin BOTH edges. The widening ones would pass under a naive
//! `NotFound => empty` patch; the two negative controls at the bottom would
//! not, and exist specifically to make that shortcut fail.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureService;
use meerkat::surface::{build_runtime_backed_service, default_persistent_executor};
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
    Session,
};
use meerkat_client::TestClient;
use meerkat_core::SessionBuildOptions;
use meerkat_core::image_generation::{
    SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin, SwitchTurnReasonTextDisposition,
    SwitchTurnRequestId,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_control::{
    SessionModelRoutingControlHistory, SessionModelRoutingControlRecord,
};
use meerkat_core::types::SessionId;
use meerkat_core::{Provider, SessionLlmIdentity, SessionToolVisibilityState};
use meerkat_runtime::{
    HydratedSessionLlmState, MeerkatMachine, ResolvedSessionLlmReconfigure, RuntimeDriverError,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};

// ---------------------------------------------------------------------------
// Real persistent service: the RPC `session/create` shape
// ---------------------------------------------------------------------------

async fn build_service(
    root: &std::path::Path,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "handoff-read-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let factory = AgentFactory::new(root.join("sessions"));
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(TestClient::for_provider(Provider::OpenAI)));
    let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
    (Arc::new(service), adapter)
}

fn create_request() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "gpt-5.4".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: meerkat::SystemPromptOverride::Set("handoff read contract".to_string()),
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    }
}

fn until_changed_model_intent(target: &str) -> SwitchTurnIntent {
    SwitchTurnIntent {
        target_model: ModelId::new(target),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    }
}

/// A session with no live actor must read as an EMPTY log, not a read failure.
///
/// This is the exact `session/create` shape that killed every RPC session:
/// the pre-dequeue seam runs before any actor is materialized, the live export
/// answers `NotFound`, and treating that as failure stopped the runtime loop
/// before it ever reached a provider.
#[tokio::test]
async fn a_session_without_a_live_actor_reads_as_an_empty_log() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (service, _adapter) = build_service(temp.path()).await;
    let session = Session::new();
    let session_id = session.id().clone();

    // Deliberately NOT materialized: no live actor exists for this id.
    let history = service
        .live_model_routing_control_history(&session_id)
        .await
        .expect("an unmaterialized session must not read as a failure");
    assert!(
        history.is_empty(),
        "a session that committed nothing owes nothing"
    );
}

/// The anti-widening control: no live actor must NOT mean no committed log.
///
/// A naive `NotFound => empty` patch passes the test above and fails here,
/// which is exactly why this exists. The committed record lives on the durable
/// authority, so absence of a LIVE actor must fall through to it rather than
/// being reported as nothing owed.
#[tokio::test]
async fn discarding_the_live_actor_must_not_hide_a_committed_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (service, adapter) = build_service(temp.path()).await;

    let mut session = Session::new();
    let session_id = session.id().clone();
    let request_id = SwitchTurnRequestId::new(uuid::Uuid::from_bytes([7u8; 16]));
    session
        .append_model_routing_control_record(
            SessionModelRoutingControlRecord::request(
                request_id,
                RunId::new(),
                until_changed_model_intent("gpt-5.5"),
            )
            .expect("representable durable request"),
        )
        .expect("seed committed request");
    let service_for_executor = Arc::clone(&service);
    let adapter_for_executor = Arc::clone(&adapter);
    Box::pin(meerkat::surface::materialize_session(
        &service,
        &adapter,
        session,
        create_request(),
        move |session_id| {
            default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
        },
    ))
    .await
    .expect("materialize session");

    // Drop the live actor: the durable row is now the only carrier, which is
    // precisely the state the pre-dequeue seam meets between turns.
    service
        .discard_live_under_runtime_turn_boundary(&session_id)
        .await
        .expect("discard live actor");

    let history = service
        .live_model_routing_control_history(&session_id)
        .await
        .expect("committed authority must still be readable without a live actor");
    assert_eq!(
        history.records().len(),
        1,
        "the committed request must survive losing the live actor: {history:?}"
    );
    assert_eq!(history.records()[0].request_id(), &request_id);
}

// ---------------------------------------------------------------------------
// Host doubles: the Mob shape and the failure-propagation control
// ---------------------------------------------------------------------------

fn capability_surface() -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: false,
        supports_thinking: false,
        supports_reasoning: false,
        inline_video: false,
        vision: false,
        image_input: false,
        image_tool_results: false,
        supports_web_search: false,
        supports_mid_conversation_system_messages: false,
        image_generation: false,
        realtime: false,
        call_timeout_secs: None,
    }
}

fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: "host-model".to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

/// Implements only the REQUIRED surface, exactly like a real host that has not
/// yet adopted this seam.
struct MinimalHost {
    /// When set, the log read fails with a genuine (non-absence) error.
    /// When `None`, the read is left to the trait default.
    read_failure: Option<String>,
}

#[async_trait]
impl SessionLlmReconfigureHost for MinimalHost {
    async fn hydrate_session_llm_state(
        &self,
        _session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        Ok(HydratedSessionLlmState {
            current_identity: identity(),
            current_visibility_state: SessionToolVisibilityState::default(),
            current_capability_surface: Some(capability_surface()),
            capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
            base_tool_names: std::collections::BTreeSet::new(),
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        _request: &SessionLlmReconfigureRequest,
        _current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        Ok(ResolvedSessionLlmReconfigure {
            target_identity: identity(),
            target_capability_surface: capability_surface(),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        _session_id: &SessionId,
        _identity: &SessionLlmIdentity,
        _capability_surface: Option<&SessionLlmCapabilitySurface>,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        _session_id: &SessionId,
        _visibility_state: Option<SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn persist_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn discard_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn load_live_session_model_routing_control_history(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionModelRoutingControlHistory, RuntimeDriverError> {
        match self.read_failure.clone() {
            // Only the explicitly-configured failure diverges; otherwise the
            // trait default applies, which is what the refusal test pins.
            Some(reason) => Err(RuntimeDriverError::Internal(reason)),
            None => {
                SessionLlmReconfigureHost::load_live_session_model_routing_control_history(
                    &UnimplementedHost,
                    session_id,
                )
                .await
            }
        }
    }
}

/// Carries no override at all, so every defaulted method is the trait default.
struct UnimplementedHost;

#[async_trait]
impl SessionLlmReconfigureHost for UnimplementedHost {
    async fn hydrate_session_llm_state(
        &self,
        _session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        unreachable!("the refusal contract is read without hydrating")
    }

    async fn resolve_target_session_llm_identity(
        &self,
        _request: &SessionLlmReconfigureRequest,
        _current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        unreachable!("the refusal contract is read without resolving")
    }

    async fn apply_live_session_llm_identity(
        &self,
        _session_id: &SessionId,
        _identity: &SessionLlmIdentity,
        _capability_surface: Option<&SessionLlmCapabilitySurface>,
    ) -> Result<(), RuntimeDriverError> {
        unreachable!("the refusal contract is read without applying")
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        _session_id: &SessionId,
        _visibility_state: Option<SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        unreachable!("the refusal contract is read without applying visibility")
    }

    async fn persist_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        unreachable!("the refusal contract is read without persisting")
    }

    async fn discard_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        unreachable!("the refusal contract is read without discarding")
    }
}

async fn machine_with_host(
    host: Arc<dyn SessionLlmReconfigureHost>,
) -> (Arc<MeerkatMachine>, SessionId) {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    adapter.set_session_llm_reconfigure_host(host);
    let session_id = SessionId::new();
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register session");
    (adapter, session_id)
}

/// The fail-closed contract: a host that never implemented the log read must
/// REFUSE, not report an empty log.
///
/// This is the inverse of the earlier draft, and the ruling that produced it
/// is the important part: defaulting to "nothing owed" would let a real host
/// that merely forgot this seam strand a committed handoff forever, silently.
/// Refusing keeps that host loud. Test doubles and hosts that genuinely owe
/// nothing declare the empty log explicitly instead (see
/// `RecordingSessionLlmReconfigureHost` in meerkat-mob).
#[tokio::test]
async fn a_host_that_never_implemented_the_log_read_still_refuses() {
    let host: Arc<dyn SessionLlmReconfigureHost> = Arc::new(MinimalHost { read_failure: None });
    let (adapter, session_id) = machine_with_host(host).await;

    let error = adapter
        .committed_model_routing_handoffs_awaiting_decision(&session_id)
        .await
        .expect_err("the unimplemented read contract must stay fail-closed");
    assert!(
        error
            .to_string()
            .contains("does not expose the model-routing handoff log"),
        "the refusal must name the missing capability: {error}"
    );
}

/// The failure-propagation control: a REAL read failure must stay a failure.
///
/// Without this, the widening above could be "simplified" into swallowing
/// every error from the log read, which would let the seam serve the next
/// input under an identity a committed handoff had already replaced.
#[tokio::test]
async fn a_genuine_log_read_failure_still_propagates() {
    let host: Arc<dyn SessionLlmReconfigureHost> = Arc::new(MinimalHost {
        read_failure: Some("durable log read exploded".to_string()),
    });
    let (adapter, session_id) = machine_with_host(host).await;

    let error = adapter
        .committed_model_routing_handoffs_awaiting_decision(&session_id)
        .await
        .expect_err("a genuine read failure must not be reported as nothing owed");
    assert!(
        error.to_string().contains("durable log read exploded"),
        "the underlying failure must survive classification: {error}"
    );
}
