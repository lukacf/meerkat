use meerkat_integration_tests::e2e_lanes::{run_catalog_scenario, run_named_suite};

macro_rules! live_scenario {
    ($name:ident, $id:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-live"]
        async fn $name() {
            run_catalog_scenario($id)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

macro_rules! live_suite {
    ($name:ident, $suite:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-live"]
        async fn $name() {
            run_named_suite($suite)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

live_scenario!(e2e_live_s15_rpc_full_lifecycle_and_recall, 15);
live_scenario!(e2e_live_s17_rpc_multi_turn_event_streaming, 17);
live_scenario!(e2e_live_s18_rpc_config_capabilities_and_errors, 18);
live_scenario!(e2e_live_s19_rpc_context_injection_recall, 19);
live_scenario!(e2e_live_s20_rpc_dedicated_event_stream_roundtrip, 20);
live_scenario!(e2e_live_s23_rest_sse_follow_up_event_stream, 23);
live_scenario!(e2e_live_s24_rest_config_capabilities_health_and_skills, 24);
live_scenario!(e2e_live_s25_rest_reload_and_resume_same_realm_root, 25);
live_scenario!(e2e_live_s26_cli_run_and_resume_persistence, 26);
live_scenario!(e2e_live_s29_cli_mob_member_turn_probe, 29);
live_scenario!(e2e_live_s31_mcp_stdio_run_and_resume_lifecycle, 31);
live_scenario!(e2e_live_s32_mcp_stdio_config_capabilities_and_skills, 32);
live_scenario!(e2e_live_s33_mcp_stdio_event_stream_roundtrip, 33);
live_scenario!(
    e2e_live_s34_mcp_streamable_http_run_and_resume_lifecycle,
    34
);
live_scenario!(
    e2e_live_s35_mcp_streamable_http_config_capabilities_and_skills,
    35
);
live_scenario!(
    e2e_live_s36_mcp_streamable_http_event_stream_and_archive,
    36
);
live_scenario!(e2e_live_s37_python_sdk_full_lifecycle_and_capabilities, 37);
live_scenario!(e2e_live_s38_python_sdk_context_injection_and_streaming, 38);
live_scenario!(
    e2e_live_s39_python_sdk_persistent_reconnect_and_runtime_accept,
    39
);
live_scenario!(e2e_live_s41_typescript_sdk_full_lifecycle_and_recall, 41);
live_scenario!(
    e2e_live_s42_typescript_sdk_deferred_context_and_streaming,
    42
);
live_scenario!(
    e2e_live_s43_typescript_sdk_persistent_reconnect_and_resume,
    43
);
live_scenario!(e2e_live_s45_browser_raw_session_lifecycle, 45);
live_scenario!(e2e_live_s46_browser_raw_session_recall, 46);
live_scenario!(e2e_live_s65_realtime_stale_attachment_authority, 65);
live_scenario!(e2e_live_s66_realtime_reconnect_budget_exhaustion, 66);
live_scenario!(e2e_live_s67_realtime_unsupported_explicit_commit, 67);
live_scenario!(e2e_live_s68_realtime_observer_fanout_and_read_only, 68);
live_scenario!(
    e2e_live_s69_realtime_disconnect_discards_staged_transcript,
    69
);
live_scenario!(e2e_live_s70_realtime_tool_continuation_failure_path, 70);
live_suite!(e2e_live_agent_mob_tools_suite, "agent-mob-tools");
live_suite!(
    e2e_live_cli_mob_rpc_state_machine_probe,
    "cli-mob-rpc-state-machine-probe"
);
live_suite!(e2e_live_cli_structured_output, "cli-structured-output");
live_suite!(
    e2e_live_provider_tool_schema_compat,
    "provider-tool-schema-compat"
);
live_suite!(
    e2e_live_surface_build_fixtures,
    "surface-build-fixtures-live"
);

/// Live row of the provider-neutral `reasoning_effort` level on the direct
/// provider APIs: one cell per provider whose key is set (OpenAI Responses,
/// Anthropic Messages, Gemini `generateContent`). A cell takes the first
/// catalog model whose row states effort levels, its provider's default
/// model first, and sends ONE request for it: a streamed turn at one level
/// of the row, with a tool offered, through a session the factory builds on
/// the provider's own client. The run must report the level with the route
/// and `facts: catalog`. Before that, and without any request, a level
/// outside the row and a catalog model whose row states no levels must be
/// refused.
///
/// The rules are those of `meerkat_integration_tests::live_reasoning_matrix`,
/// pinned there by rows that need no provider: at most one request per cell,
/// counted by a gate in front of the client; the run ends at the first 401,
/// 403, 429, exhausted quota or policy stop and sends nothing after it; a
/// line carries a failure's class and HTTP status, never the provider's
/// text; every cell the run reaches is also printed on stdout as one
/// `reasoning-effort-receipt-observation` line of typed fields for the
/// reasoning effort execution receipt. A run sends at most one request per
/// provider with a key, three at most, and asks for nothing else: a direct
/// route has no account lookup, and a direct client sends nothing a second
/// time on its own.
///
/// ```text
/// repo-cargo test -p meerkat-integration-tests --test e2e_live_lane \
///     direct_provider_live_reasoning_effort -- --ignored --exact --nocapture
/// ```
///
/// Without a provider key the row prints a skip and passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:e2e-live"]
async fn direct_provider_live_reasoning_effort() {
    use std::time::{Duration, Instant};

    use meerkat::AgentFactory;
    use meerkat_core::{Config, Provider, ReasoningLevelFacts, SessionLlmIdentity};
    use meerkat_integration_tests::live_reasoning_matrix::{
        self as matrix, CellModel, FailureClass, Lane, MatrixLine, MatrixRun, Redactor,
        RequestBudget,
    };

    /// The variables a provider's key is read from, as the live lane names
    /// them.
    const PROVIDER_KEYS: [(Provider, &[&str]); 3] = [
        (Provider::OpenAI, &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]),
        (
            Provider::Anthropic,
            &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
        ),
        (
            Provider::Gemini,
            &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
        ),
    ];

    let providers: Vec<Provider> = PROVIDER_KEYS
        .iter()
        .filter(|(_, names)| {
            names
                .iter()
                .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
        })
        .map(|(provider, _)| *provider)
        .collect();
    if providers.is_empty() {
        eprintln!("SKIP direct-live reasoning: no provider key is set");
        return;
    }

    let mut config = Config::default();
    config.model_fallback.enabled = Some(false);
    let temp = tempfile::TempDir::new().unwrap_or_else(|error| panic!("a directory: {error}"));
    let factory = AgentFactory::new(temp.path().join("factory-store"))
        .runtime_root(temp.path().join("runtime"));
    let redactor = Redactor::default();

    // Per provider: the first catalog model whose row states levels on the
    // direct route and the first whose row states none.
    let mut candidates = Vec::new();
    for provider in providers {
        let observation = meerkat_core::direct_route_observation(provider);
        let facts = |model: &str| {
            meerkat_core::supported_reasoning_levels(
                provider,
                observation.as_ref(),
                meerkat_models::capabilities_for(provider, model),
            )
        };
        let models: Vec<&str> = meerkat_models::default_model(provider)
            .into_iter()
            .chain(meerkat_models::allowed_models(provider))
            .collect();
        let with_levels = models.iter().copied().find(|model| {
            matches!(facts(model), ReasoningLevelFacts::Stated { levels, .. } if !levels.is_empty())
        });
        let without_levels = models
            .iter()
            .copied()
            .find(|model| matches!(facts(model), ReasoningLevelFacts::Unknown));
        for model in with_levels.into_iter().chain(without_levels) {
            let identity = SessionLlmIdentity {
                model: model.to_string(),
                provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            };
            let Ok(client) = factory
                .build_llm_client_for_identity(&config, &identity)
                .await
            else {
                let line = MatrixLine::Fail {
                    route: format!("{}/-", provider.as_str()),
                    model: model.to_string(),
                    shape: None,
                    level: None,
                    class: FailureClass::BuildFailed,
                    status: None,
                    detail: Some("route_client"),
                }
                .render(Lane::Direct);
                eprintln!("{}", redactor.redact(&line));
                panic!("STOP: a provider client did not build; no request was sent");
            };
            candidates.push((
                provider,
                CellModel {
                    model: model.to_string(),
                    client,
                    auth_binding: None,
                    config: config.clone(),
                },
            ));
        }
    }
    let cells = matrix::assemble_cells(candidates);

    let run = MatrixRun {
        lane: Lane::Direct,
        state_root: temp.path().join("matrix"),
        budget: RequestBudget::new(matrix::REQUESTS_PER_CELL),
        redactor,
        turn_timeout: Duration::from_secs(90),
        deadline: Instant::now() + Duration::from_secs(240),
        max_tokens: 512,
        max_retries: 0,
    };
    let summary = matrix::run_matrix(&run, &cells, &matrix::print_line).await;

    assert!(
        summary.stopped.is_none(),
        "STOP: a provider failure ended the run; see its STOP line"
    );
    assert_eq!(summary.fail, 0, "a cell failed; see its FAIL line");
    assert!(
        summary.pass > 0,
        "no cell passed: no provider with a key has a catalog model that states levels, or every turn was cut"
    );
}
