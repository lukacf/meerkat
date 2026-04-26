use meerkat_integration_tests::e2e_lanes::run_named_suite;
use std::sync::OnceLock;
use tokio::sync::Mutex;

fn system_lane_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

macro_rules! system_suite {
    ($name:ident, $suite:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "lane:e2e-system"]
        async fn $name() {
            let _guard = system_lane_lock().lock().await;
            run_named_suite($suite)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    };
}

system_suite!(e2e_system_cli_init_snapshot, "cli-init-snapshot");
system_suite!(e2e_system_cli_resume_tools, "cli-resume-tools");
system_suite!(e2e_system_rest_resume_metadata, "rest-resume-metadata");
system_suite!(
    e2e_system_cli_capabilities_and_config,
    "cli-capabilities-and-config"
);
system_suite!(
    e2e_system_cli_mobpack_pack_inspect_validate,
    "cli-mobpack-pack-inspect-validate"
);
system_suite!(e2e_system_cli_wasm_surface_gate, "cli-wasm-surface-gate");
system_suite!(
    e2e_system_cli_wasm_forbidden_capability,
    "cli-wasm-forbidden-capability"
);
system_suite!(
    e2e_system_sqlite_shared_realm_rpc_rest_rpc,
    "sqlite-shared-realm-rpc-rest-rpc"
);
system_suite!(
    e2e_system_sqlite_shared_realm_cli_rpc_cli,
    "sqlite-shared-realm-cli-rpc-cli"
);
system_suite!(
    e2e_system_sqlite_shared_realm_cli_rest_cli,
    "sqlite-shared-realm-cli-rest-cli"
);
