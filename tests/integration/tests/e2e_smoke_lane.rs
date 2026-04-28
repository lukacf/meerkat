use meerkat_integration_tests::e2e_lanes::{run_catalog_scenario, run_named_suite};

macro_rules! smoke_scenario {
    ($name:ident, $id:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-smoke"]
        async fn $name() {
            run_catalog_scenario($id)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

macro_rules! smoke_suite {
    ($name:ident, $suite:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-smoke"]
        async fn $name() {
            run_named_suite($suite)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

smoke_scenario!(e2e_smoke_s16_rpc_kitchen_sink, 16);
smoke_scenario!(e2e_smoke_s21_rpc_mob_callback_tools, 21);
smoke_scenario!(e2e_smoke_s22_rpc_transport_backpressure, 22);
smoke_scenario!(e2e_smoke_s27_cli_shell_and_structured_output, 27);
smoke_scenario!(e2e_smoke_s28_cli_signed_mobpack_deploy, 28);
smoke_scenario!(e2e_smoke_s30_cli_mob_flow_probe, 30);
smoke_scenario!(e2e_smoke_s40_python_sdk_mixed_provider_swarm_probe, 40);
smoke_scenario!(e2e_smoke_s44_typescript_sdk_mixed_provider_swarm_probe, 44);
smoke_scenario!(e2e_smoke_s47_browser_mobpack_session_flow, 47);
smoke_scenario!(e2e_smoke_s48_browser_raw_mob_lifecycle, 48);
smoke_scenario!(e2e_smoke_s49_rpc_rest_shared_realm_roundtrip, 49);
smoke_scenario!(e2e_smoke_s50_rest_cli_shared_realm_roundtrip, 50);
smoke_scenario!(e2e_smoke_s51_rpc_mcp_shared_realm_parity, 51);
smoke_scenario!(e2e_smoke_s52_cli_rpc_cli_continuity, 52);
smoke_scenario!(e2e_smoke_s53_cli_rest_cli_continuity, 53);
smoke_scenario!(e2e_smoke_s54_shared_realm_mob_visibility, 54);
smoke_scenario!(e2e_smoke_s55_rpc_rest_callback_peer_storm_resume, 55);
smoke_scenario!(
    e2e_smoke_s57_python_sdk_realtime_channel_session_exchange,
    57
);
smoke_scenario!(
    e2e_smoke_s58_python_sdk_realtime_member_respawn_continuity,
    58
);
smoke_scenario!(
    e2e_smoke_s59_typescript_sdk_realtime_channel_session_exchange,
    59
);
smoke_scenario!(e2e_smoke_s60_rust_sdk_realtime_channel_session_exchange, 60);
smoke_scenario!(e2e_smoke_s61_cli_realtime_bridge_session_roundtrip, 61);
smoke_scenario!(
    e2e_smoke_s62_rest_bootstrap_to_rust_sdk_realtime_channel_exchange,
    62
);
smoke_scenario!(
    e2e_smoke_s63_mcp_bootstrap_to_rust_sdk_member_realtime_exchange,
    63
);
smoke_scenario!(
    e2e_smoke_s64_python_sdk_realtime_member_model_switch_continuity,
    64
);
smoke_scenario!(
    e2e_smoke_s71_rust_sdk_realtime_audio_mob_collaboration_roundtrip,
    71
);
smoke_scenario!(
    e2e_smoke_s72_rust_sdk_realtime_audio_member_model_switch_continuity,
    72
);
smoke_scenario!(e2e_smoke_s73_cli_generate_image_openai_default, 73);
smoke_scenario!(e2e_smoke_s74_python_sdk_gemini_image_provider_params, 74);
smoke_scenario!(
    e2e_smoke_s75_typescript_sdk_openai_image_provider_params,
    75
);
smoke_scenario!(
    e2e_smoke_s76_typescript_sdk_cross_provider_image_model_switch_stress,
    76
);
smoke_scenario!(e2e_smoke_s77_typescript_sdk_stacked_image_turn, 77);
smoke_scenario!(e2e_smoke_s78_typescript_sdk_cross_provider_image_relay, 78);
smoke_scenario!(e2e_smoke_s79_typescript_sdk_mob_image_critic, 79);
smoke_scenario!(
    e2e_smoke_s80_typescript_sdk_persisted_generated_image_resume,
    80
);
smoke_scenario!(e2e_smoke_s81_typescript_sdk_parallel_image_storm, 81);
smoke_scenario!(e2e_smoke_s82_typescript_sdk_blob_image_roundtrip, 82);
smoke_suite!(e2e_smoke_rpc_dynamic_tool_pickup, "rpc-dynamic-tool-pickup");
smoke_suite!(
    e2e_smoke_rpc_deferred_catalog_session,
    "rpc-deferred-catalog-session"
);
smoke_suite!(
    e2e_smoke_cli_background_job_active_turn,
    "cli-background-job-active-turn"
);
smoke_suite!(
    e2e_smoke_cli_background_job_idle_keepalive,
    "cli-background-job-idle-keepalive"
);
smoke_suite!(e2e_smoke_mob_live_smoke, "mob-live-smoke");
smoke_suite!(e2e_smoke_mob_flow_runtime_suite, "mob-flow-runtime");
smoke_suite!(e2e_smoke_mob_pictionary, "mob-pictionary");
