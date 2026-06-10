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
    LiveChannelCloseFailure, LiveChannelRefreshFailure, LiveConfigPropagationReport,
    LiveHotSwapSkipReason, apply_precheck_gates, live_channel_requires_close_for_identity_change,
    should_apply_global_model_hot_swap, should_fire_live_propagation,
};
use meerkat_core::types::SessionId;
use meerkat_core::{Provider, SessionLlmIdentity};

#[test]
fn precheck_b19_fires_before_b18_for_non_realtime_non_openai() {
    let err = apply_precheck_gates(Provider::Anthropic, "claude-opus-4-8", false)
        .expect_err("non-realtime should fail B19");
    match err {
        LiveOpenPrecheckError::ModelNotRealtime { model, provider } => {
            assert_eq!(model, "claude-opus-4-8");
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
        &prev, &next
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
        &prev, &next
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
        &identity, &identity
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

// ---------------------------------------------------------------------------
// `LiveConfigPropagationReport` is the typed aggregate that
// `propagate_config_to_live_channels` returns to the config/patch caller.
// It replaces the prior pure-logging fan-out: a per-channel refresh that
// fails (open_config build / snapshot stamp / enqueue / queue acceptance)
// or a per-session hot-swap that fails must be enumerated in the report so
// the caller observes the failure instead of relying on tracing to notice a
// propagation that silently completed. These tests pin that contract
// independently of the host-backed fan-out (which lands richer coverage in
// `orchestrator_e2e` once a fault-injecting host fixture exists).
// ---------------------------------------------------------------------------

#[test]
fn propagation_report_default_is_clean_and_empty() {
    // No host / nothing to propagate → an empty report that reads clean.
    let report = LiveConfigPropagationReport::default();
    assert!(report.is_clean());
    assert!(report.swapped.is_empty());
    assert!(report.skipped.is_empty());
    assert!(report.swap_failed.is_empty());
    assert!(report.refreshed.is_empty());
    assert!(report.closed.is_empty());
    assert!(report.refresh_failed.is_empty());
}

#[test]
fn propagation_report_with_forced_channel_failure_is_not_clean_and_enumerates_it() {
    // Part 2 gate: a propagation run with a forced per-channel failure
    // must return a report enumerating the failure rather than silently
    // completing. A refresh that could not be enqueued is recorded as a
    // typed `LiveChannelRefreshFailure::EnqueueFailed`, and `is_clean()`
    // flips false so the caller can react.
    let refreshed = SessionId::new();
    let failed = SessionId::new();
    let report = LiveConfigPropagationReport {
        refreshed: vec![refreshed.clone()],
        refresh_failed: vec![(
            failed.clone(),
            LiveChannelRefreshFailure::EnqueueFailed("live channel closed".to_string()),
        )],
        ..Default::default()
    };

    assert!(
        !report.is_clean(),
        "a dropped per-channel refresh must not read as a clean propagation"
    );
    assert_eq!(report.refreshed, vec![refreshed]);
    assert_eq!(report.refresh_failed.len(), 1);
    let (session, failure) = &report.refresh_failed[0];
    assert_eq!(session, &failed);
    match failure {
        LiveChannelRefreshFailure::EnqueueFailed(detail) => {
            assert_eq!(detail, "live channel closed");
        }
        other => panic!("expected EnqueueFailed, got {other:?}"),
    }
}

#[test]
fn propagation_report_with_swap_failure_is_not_clean() {
    // A per-session hot-swap reconfigure failure must also surface in the
    // report (not be swallowed) and flip `is_clean()` false.
    let failed = SessionId::new();
    let report = LiveConfigPropagationReport {
        swap_failed: vec![(failed.clone(), "reconfigure rejected".to_string())],
        ..Default::default()
    };
    assert!(!report.is_clean());
    assert_eq!(report.swap_failed.len(), 1);
    assert_eq!(report.swap_failed[0].0, failed);
}

#[test]
fn propagation_report_deliberate_skip_and_close_stay_clean() {
    // A deliberate no-op/override skip and an intentional channel close are
    // expected outcomes, not faults: the report still reads clean so the
    // caller does not treat a correct propagation as a failure.
    let skipped = SessionId::new();
    let closed = SessionId::new();
    let report = LiveConfigPropagationReport {
        skipped: vec![(skipped, LiveHotSwapSkipReason::NoOpOrOverride)],
        closed: vec![closed],
        ..Default::default()
    };
    assert!(report.is_clean());
}

#[test]
fn propagation_report_with_close_failure_is_not_clean() {
    // A config-rejection close that itself failed must surface as a typed
    // `LiveChannelCloseFailure` in `close_failed` (NOT in `closed`) and flip
    // `is_clean()` false — a failed close must never be laundered into a
    // clean propagation.
    let failed = SessionId::new();
    let report = LiveConfigPropagationReport {
        close_failed: vec![(
            failed.clone(),
            LiveChannelCloseFailure::CommitHandoffMissing,
        )],
        ..Default::default()
    };
    assert!(!report.is_clean());
    assert!(report.closed.is_empty());
    assert_eq!(report.close_failed.len(), 1);
    assert_eq!(report.close_failed[0].0, failed);
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
        StagedSessionRegistry, StagedSlot,
    };
    use meerkat_core::SessionLlmIdentity;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;
    use meerkat_store::MemoryBlobStore;

    struct Fixture {
        service: Arc<meerkat_session::PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        archive_runtime_cleanup: ArchiveRuntimeCleanup,
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
        StagedSlot::new_staged(
            &SessionId::new(),
            build_config,
            SessionLlmIdentity {
                model: model.to_string(),
                provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            None,
            None,
            now,
            now,
            false,
        )
        .expect("staged slot fixture must satisfy generated staged-session authority")
    }

    /// `propagate_config_to_live_channels` must short-circuit when no
    /// live-adapter host is attached. Without the early return the
    /// orchestrator would unconditionally enumerate channels and
    /// dispatch refresh commands against the missing host.
    #[tokio::test]
    async fn propagate_config_to_live_channels_no_host_is_noop() {
        let fx = build_fixture();
        let orch = orchestrator(&fx);
        // Reaches the early-return branch and returns an empty, clean report.
        let report = orch.propagate_config_to_live_channels(None).await;
        assert!(report.is_clean());
        assert_eq!(report, Default::default());
    }

    /// `precheck_live_open` must reject a deferred staged session whose
    /// effective LLM identity is not realtime-capable. The catalog
    /// flags claude-opus-4-8 (Anthropic) as non-realtime, so the B19
    /// gate fires (B18 is the no-OpenAI gate but B19 is more
    /// specific).
    #[tokio::test]
    async fn precheck_live_open_rejects_non_realtime_staged_session() {
        let fx = build_fixture();
        let orch = orchestrator(&fx);

        let session_id = SessionId::new();
        let slot = staged_slot("claude-opus-4-8", meerkat_core::Provider::Anthropic);
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
                assert_eq!(model, "claude-opus-4-8");
                assert_eq!(provider, "anthropic");
            }
            other => panic!("expected ModelNotRealtime, got {other:?}"),
        }
    }
}
