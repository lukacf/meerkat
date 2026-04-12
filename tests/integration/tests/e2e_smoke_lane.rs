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
smoke_suite!(e2e_smoke_rpc_mob_callback_tools, "rpc-mob-callback-tools");
smoke_suite!(
    e2e_smoke_rpc_transport_backpressure,
    "rpc-transport-backpressure"
);
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
