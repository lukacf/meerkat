use meerkat_integration_tests::e2e_lanes::run_named_suite;

macro_rules! build_suite {
    ($name:ident, $suite:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-build"]
        async fn $name() {
            run_named_suite($suite)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

build_suite!(e2e_build_fixture_embedded_min, "fixture-embedded-min");
build_suite!(
    e2e_build_fixture_runtime_backed_min,
    "fixture-runtime-backed-min"
);
build_suite!(
    e2e_build_surface_rkat_deterministic,
    "surface-rkat-deterministic"
);
build_suite!(
    e2e_build_surface_rkat_mini_deterministic,
    "surface-rkat-mini-deterministic"
);
build_suite!(
    e2e_build_surface_rkat_rpc_deterministic,
    "surface-rkat-rpc-deterministic"
);
build_suite!(
    e2e_build_surface_rkat_rpc_mini_deterministic,
    "surface-rkat-rpc-mini-deterministic"
);
build_suite!(
    e2e_build_surface_rkat_rest_deterministic,
    "surface-rkat-rest-deterministic"
);
build_suite!(
    e2e_build_surface_rkat_mcp_deterministic,
    "surface-rkat-mcp-deterministic"
);
