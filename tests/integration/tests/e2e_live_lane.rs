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
    e2e_live_surface_build_fixtures,
    "surface-build-fixtures-live"
);
