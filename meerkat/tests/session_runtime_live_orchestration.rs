//! Smoke test for `meerkat::session_runtime::live_orchestration`.
//!
//! Covers the surface-agnostic helpers moved out of
//! `meerkat-rpc::session_runtime`:
//!
//! * `apply_precheck_gates` — B19 (realtime capability) fires before B18
//!   (provider has live adapter); a non-realtime non-OpenAI session
//!   reports `ModelNotRealtime` (the more specific failure), not
//!   `ProviderHasNoLiveAdapter`. This pins the gate ordering as
//!   intentional contract — the catalog cannot naturally produce a
//!   realtime-capable non-OpenAI row, so the synthetic
//!   `realtime_capable: true` argument is the only way to exercise the
//!   B18 branch.
//! * `live_channel_requires_close_for_identity_change` — closes on
//!   model swap, closes on provider swap, in-place refresh otherwise.
//! * `extract_system_prompt_from_seed_messages_runtime` — surfaces the
//!   first `System`/`SystemNotice` lead.
//!
//! Coverage of the load-bearing methods (`precheck_live_open`,
//! `materialize_staged_session_for_realtime_open`,
//! `propagate_config_to_live_channels`) lives in `meerkat-rpc`'s
//! integration tests today; once W3-B promotes the RPC accessors
//! upstream, those methods land on `LiveOrchestrator<'a>` and gain
//! direct coverage here.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat::session_runtime::errors::LiveOpenPrecheckError;
use meerkat::session_runtime::live_orchestration::{
    apply_precheck_gates, extract_system_prompt_from_seed_messages_runtime,
    live_channel_requires_close_for_identity_change, should_apply_global_model_hot_swap,
    should_fire_live_propagation,
};
use meerkat_core::types::{Message, SystemMessage, SystemNoticeKind, SystemNoticeMessage};
use meerkat_core::{Provider, SessionLlmIdentity};

#[test]
fn precheck_b19_fires_before_b18_for_non_realtime_non_openai() {
    let err = apply_precheck_gates(Provider::Anthropic, "claude-opus-4-7", false)
        .expect_err("non-realtime should fail B19");
    match err {
        LiveOpenPrecheckError::ModelNotRealtime { model, provider } => {
            assert_eq!(model, "claude-opus-4-7");
            assert_eq!(provider, "anthropic");
        }
        other => panic!("expected ModelNotRealtime, got {other:?}"),
    }
}

#[test]
fn precheck_b18_rejects_realtime_capable_non_openai() {
    let err = apply_precheck_gates(Provider::Anthropic, "synthetic-rt-anthropic", true)
        .expect_err("realtime-capable non-OpenAI should fail B18");
    match err {
        LiveOpenPrecheckError::ProviderHasNoLiveAdapter { provider } => {
            assert_eq!(provider, "anthropic");
        }
        other => panic!("expected ProviderHasNoLiveAdapter, got {other:?}"),
    }
}

#[test]
fn precheck_accepts_realtime_capable_openai() {
    apply_precheck_gates(Provider::OpenAI, "gpt-realtime-2", true)
        .expect("realtime-capable OpenAI must pass both gates");
}

#[test]
fn live_channel_close_on_model_swap() {
    let prev = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let next = SessionLlmIdentity {
        model: "gpt-realtime-3".into(),
        ..prev.clone()
    };
    assert!(live_channel_requires_close_for_identity_change(
        Some(&prev),
        &next
    ));
}

#[test]
fn live_channel_close_on_provider_swap() {
    let prev = SessionLlmIdentity {
        model: "shared".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let next = SessionLlmIdentity {
        provider: Provider::Anthropic,
        ..prev.clone()
    };
    assert!(live_channel_requires_close_for_identity_change(
        Some(&prev),
        &next
    ));
}

#[test]
fn live_channel_in_place_refresh_when_identity_unchanged() {
    let identity = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    assert!(!live_channel_requires_close_for_identity_change(
        Some(&identity),
        &identity
    ));
}

#[test]
fn live_channel_no_close_when_no_bound_identity() {
    let next = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    assert!(!live_channel_requires_close_for_identity_change(
        None, &next
    ));
}

// ---------------------------------------------------------------------------
// `should_apply_global_model_hot_swap` is the pure rule that decides
// whether `propagate_config_to_live_channels` should hot-swap a session
// to the new global model. The rule is now:
//
//   Skip when current_session_model == new_global_model (no-op);
//   otherwise propagate.
//
// `prior_global_model` is no longer consulted — the original G5 (P1)
// rule attempted to preserve "per-session overrides" by skipping when
// `current` differed from `prior_global`, but that heuristic conflated
// "user pinned at session/create" with "user reconfigured mid-session"
// and broke the s72 e2e contract (a session created with an explicit
// realtime model against a non-realtime global must still re-resolve
// to the new non-realtime global so the next live/open precheck rejects
// via B19).
// ---------------------------------------------------------------------------

#[test]
fn hot_swap_propagates_when_prior_global_unknown_and_models_differ() {
    // s72 regression: even without a prior baseline, the new global
    // differs from the session's current model → propagate. The earlier
    // "skip when prior is None" rule left stale-tracking sessions
    // stranded; the new rule trusts the new global as authoritative.
    assert!(should_apply_global_model_hot_swap(
        "gpt-realtime-2",
        None,
        "gpt-realtime-3"
    ));
}

#[test]
fn hot_swap_skips_when_session_already_matches_new_global() {
    // Session model already equals new global → hot-swap would be a
    // no-op; the per-channel Refresh fan-out below still runs.
    assert!(!should_apply_global_model_hot_swap(
        "gpt-realtime-3",
        Some("gpt-realtime-2"),
        "gpt-realtime-3"
    ));
    // Same outcome regardless of prior-baseline knowledge.
    assert!(!should_apply_global_model_hot_swap(
        "gpt-realtime-3",
        None,
        "gpt-realtime-3"
    ));
}

#[test]
fn hot_swap_propagates_when_session_was_tracking_prior_global() {
    // Session was tracking the prior global → safe to retarget.
    assert!(should_apply_global_model_hot_swap(
        "gpt-realtime-2",
        Some("gpt-realtime-2"),
        "gpt-realtime-3"
    ));
}

#[test]
fn hot_swap_propagates_when_session_diverged_from_prior_global() {
    // s72 regression: session model differs from both the prior and the
    // new global. The original G5 rule treated this as an "override" and
    // skipped — that broke s72, where the test creates a session with
    // an explicit realtime model against a non-realtime global. The
    // new rule treats the global as authoritative for any value mismatch.
    assert!(should_apply_global_model_hot_swap(
        "gpt-realtime-prior-override",
        Some("gpt-realtime-2"),
        "gpt-realtime-3"
    ));
}

#[test]
fn hot_swap_skips_when_session_already_matches_new_global_after_divergence() {
    // Edge: the per-session model coincides with the new global. The
    // hot-swap is a no-op regardless of prior baseline.
    assert!(!should_apply_global_model_hot_swap(
        "gpt-realtime-3",
        Some("gpt-realtime-2"),
        "gpt-realtime-3"
    ));
}

// ---------------------------------------------------------------------------
// `should_fire_live_propagation` is the symmetric gate that both
// `config/set` and `config/patch` consult before fanning out to
// `propagate_config_to_live_channels`.
//
// Findings R3-2-4 (P1+P2):
//   - `config/patch` over-applied (fired on every patch, regardless of
//     which fields changed).
//   - `config/set` under-applied (never fired at all).
// Both now route through this helper.
//
// Field set consulted (verified against the propagate body): `agent.model`.
// If the propagate body grows, extend the helper AND these tests in
// lock-step.
// ---------------------------------------------------------------------------

#[test]
fn should_fire_live_propagation_returns_false_for_unrelated_field_change() {
    // P1 regression: tools/skills/anything-else patch must NOT trigger
    // a live-channel fan-out. Mutating an unrelated field on the config
    // (here: `max_tokens`) must leave the predicate false.
    let prior = meerkat_core::config::Config::default();
    let mut new = prior.clone();
    new.max_tokens = prior.max_tokens.saturating_add(1);
    assert_ne!(prior.max_tokens, new.max_tokens);
    assert_eq!(prior.agent.model, new.agent.model);
    assert!(!should_fire_live_propagation(&prior, &new));
}

#[test]
fn should_fire_live_propagation_returns_true_when_agent_model_changes() {
    // The canonical positive case: a `config/patch agent.model` (or a
    // `config/set` that swaps the model) must fan out so live channels
    // re-resolve and either Refresh or Close.
    let prior = meerkat_core::config::Config::default();
    let mut new = prior.clone();
    new.agent.model = "gpt-realtime-3".to_string();
    assert_ne!(prior.agent.model, new.agent.model);
    assert!(should_fire_live_propagation(&prior, &new));
}

#[test]
fn should_fire_live_propagation_returns_false_when_agent_model_unchanged() {
    // Identity case: nothing changed → no propagate. The per-session
    // hot-swap loop and per-channel Refresh are entirely skipped.
    let prior = meerkat_core::config::Config::default();
    let new = prior.clone();
    assert!(!should_fire_live_propagation(&prior, &new));
}

#[test]
fn should_fire_live_propagation_only_consults_agent_model_today() {
    // Pin the field set: if someone wires in a new propagate-affecting
    // field without updating the helper, this test still passes (the
    // helper correctly returns false on the model-equality case), but
    // adding the new field to the helper without updating its
    // doc-comment will be caught by the assertion below — flipping
    // `agent.system_prompt` (a non-propagate-affecting field today)
    // must NOT fire propagation. If the propagate body grows to
    // consult `agent.system_prompt`, update the helper AND replace
    // this assertion with the matching positive case.
    let prior = meerkat_core::config::Config::default();
    let mut new = prior.clone();
    new.agent.system_prompt = Some("flipped".to_string());
    assert_eq!(prior.agent.model, new.agent.model);
    assert!(!should_fire_live_propagation(&prior, &new));
}

#[test]
fn extract_system_prompt_returns_system_message_content() {
    let msgs = vec![
        Message::System(SystemMessage::new("you are helpful")),
        Message::User(meerkat_core::types::UserMessage::text("hi")),
    ];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        Some("you are helpful".to_string())
    );
}

#[test]
fn extract_system_prompt_returns_rendered_notice_text() {
    let notice = SystemNoticeMessage::new(SystemNoticeKind::McpPending, "MCP servers connecting");
    let rendered = notice.rendered_text();
    let msgs = vec![Message::SystemNotice(notice)];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        Some(rendered)
    );
}

#[test]
fn extract_system_prompt_none_when_first_is_user() {
    let msgs = vec![Message::User(meerkat_core::types::UserMessage::text("hi"))];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        None
    );
}

// Phase 4 R1: end-to-end coverage of the load-bearing methods now
// hosted on `LiveOrchestrator<'a>`. The orchestrator borrows resolved
// state from the calling surface, so each test stands up a minimal
// session-store-backed runtime and constructs an orchestrator directly
// (no RPC SessionRuntime in sight). RPC's R11 tests in
// `meerkat-rpc/src/session_runtime.rs::tests` continue to cover the
// thin shim path.
#[cfg(all(
    feature = "session-store",
    feature = "live",
    feature = "memory-store",
    not(target_arch = "wasm32")
))]
mod orchestrator_e2e {
    use std::sync::Arc;

    use meerkat::session_runtime::admission::StagedCapacityAdmissions;
    use meerkat::session_runtime::errors::LiveOpenPrecheckError;
    use meerkat::session_runtime::live_orchestration::LiveOrchestrator;
    use meerkat::session_runtime::runtime_state::ArchiveRuntimeCleanup;
    use meerkat::surface::build_runtime_backed_service_with_capacities;
    use meerkat::{
        AgentBuildConfig, AgentFactory, Config, FactoryAgentBuilder, PersistenceBundle,
        StagedPhase, StagedSessionRegistry, StagedSlot,
    };
    use meerkat_core::SessionLlmIdentity;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::{
        HydratedSessionLlmState, MeerkatMachine, ResolvedSessionLlmReconfigure, RuntimeDriverError,
        SessionLlmReconfigureHost, SessionLlmReconfigureRequest,
    };
    use meerkat_store::MemoryBlobStore;

    /// Smallest possible reconfigure host: every operation is rejected
    /// with `RuntimeDriverError::Internal`. Used by the orchestrator
    /// `propagate_config_to_live_channels` no-op test where no live
    /// channels exist, so the host is never queried.
    struct UnusedReconfigureHost;

    #[async_trait::async_trait]
    impl SessionLlmReconfigureHost for UnusedReconfigureHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current: &SessionLlmIdentity,
        ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _identity: &SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            _state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal("UnusedReconfigureHost".into()))
        }
    }

    struct Fixture {
        service: Arc<meerkat_session::PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        archive_runtime_cleanup: ArchiveRuntimeCleanup,
        reconfigure_host: UnusedReconfigureHost,
        runtime_adapter: Arc<MeerkatMachine>,
        _temp: tempfile::TempDir,
    }

    fn build_fixture() -> Fixture {
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence =
            PersistenceBundle::new(session_store, None, Arc::new(MemoryBlobStore::new()));
        let temp = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let staged_sessions = Arc::new(StagedSessionRegistry::new());
        let (service, runtime_adapter) =
            build_runtime_backed_service_with_capacities(builder, 4, 16, persistence);
        let service = Arc::new(service);
        let staged_capacity_admissions: StagedCapacityAdmissions =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let archive_runtime_cleanup = ArchiveRuntimeCleanup {
            runtime_adapter: Arc::clone(&runtime_adapter),
            pending_session_event_streams: None,
            mcp_state: None,
            mob_state: None,
        };
        Fixture {
            service,
            staged_sessions,
            staged_capacity_admissions,
            archive_runtime_cleanup,
            reconfigure_host: UnusedReconfigureHost,
            runtime_adapter,
            _temp: temp,
        }
    }

    fn orchestrator(fx: &Fixture) -> LiveOrchestrator<'_> {
        LiveOrchestrator {
            service: &fx.service,
            staged_sessions: &fx.staged_sessions,
            staged_capacity_admissions: &fx.staged_capacity_admissions,
            runtime_adapter: &fx.runtime_adapter,
            host: None,
            config_runtime: None,
            default_llm_client: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            archive_runtime_cleanup: fx.archive_runtime_cleanup.clone(),
            llm_reconfigure_host: &fx.reconfigure_host,
            realm_id: None,
            instance_id: None,
            backend: None,
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn staged_slot(model: &str, provider: meerkat_core::Provider) -> StagedSlot {
        let mut build_config = AgentBuildConfig::new(model.to_string());
        build_config.provider = Some(provider);
        let now = now_secs();
        StagedSlot {
            phase: StagedPhase::Staged {
                build_config: Box::new(build_config),
            },
            effective_llm_identity: SessionLlmIdentity {
                model: model.to_string(),
                provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            labels: None,
            deferred_prompt: None,
            created_at_secs: now,
            updated_at_secs: now,
            machine_archived_resume_authorized: false,
        }
    }

    /// `propagate_config_to_live_channels` must short-circuit when no
    /// live-adapter host is attached. Without the early return the
    /// orchestrator would unconditionally enumerate channels and
    /// dispatch refresh commands against the missing host.
    #[tokio::test]
    async fn propagate_config_to_live_channels_no_host_is_noop() {
        let fx = build_fixture();
        let orch = orchestrator(&fx);
        // Reaches the early-return branch and does not panic.
        orch.propagate_config_to_live_channels(None).await;
    }

    /// `precheck_live_open` must reject a deferred staged session whose
    /// effective LLM identity is not realtime-capable. The catalog
    /// flags claude-opus-4-7 (Anthropic) as non-realtime, so the B19
    /// gate fires (B18 is the no-OpenAI gate but B19 is more
    /// specific).
    #[tokio::test]
    async fn precheck_live_open_rejects_non_realtime_staged_session() {
        let fx = build_fixture();
        let orch = orchestrator(&fx);

        let session_id = SessionId::new();
        let slot = staged_slot("claude-opus-4-7", meerkat_core::Provider::Anthropic);
        fx.staged_sessions
            .stage(session_id.clone(), slot)
            .await
            .expect("stage deferred session");

        let err = orch
            .precheck_live_open(&session_id)
            .await
            .expect_err("non-realtime staged session must be rejected");
        match err {
            LiveOpenPrecheckError::ModelNotRealtime { model, provider } => {
                assert_eq!(model, "claude-opus-4-7");
                assert_eq!(provider, "anthropic");
            }
            other => panic!("expected ModelNotRealtime, got {other:?}"),
        }
    }
}
