//! WorkGraph reference gateway for the console workgraph e2e
//! (`console/workgraph-e2e.cjs`).
//!
//! Boots the same library-mode runtime as `library_mode_reference`. The
//! `UnifiedRuntime` builder wires an ephemeral (memory-store) WorkGraph
//! service plus the runtime-wide admission at bootstrap - the only legal
//! wiring point; late attachment runs guard-degraded (see
//! `UnifiedRuntime::workgraph_service`). The console runs unenforced
//! (`require_app_auth: false`), so `apply_workgraph_to_experience` mirrors
//! availability into `can_view`/`can_manage` and the anonymous e2e can both
//! seed and read through `POST /console/rpc`. The graph fixture itself is
//! seeded by the e2e script over the real wire contract, not here.
//!
//! Environment:
//! - `META_WORKGRAPH_E2E_ADDR`  listen address (default `127.0.0.1:3240`)
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::TestClient;
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConsolePolicy, DiscoverySpec, MetaConfig, PreSpawnData,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime,
    build_runtime_decision_state,
};
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};

const MOB_TOML: &str = r#"
[mob]
id = "workgraph-e2e-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition = MobDefinition::from_toml(MOB_TOML)
        .map_err(|e| std::io::Error::other(format!("bad mob definition: {e}")))?;

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "workgraph-e2e".to_string(),
                    modules: vec![],
                },
                pre_spawn: Vec::<PreSpawnData>::new(),
            })
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await?;

    // The e2e is meaningless against a workgraph-less runtime: fail loudly
    // at boot instead of letting the browser test report -32041 noise.
    if runtime.workgraph_service().is_none() {
        return Err(std::io::Error::other(
            "runtime booted without a WorkGraph service; the builder wiring regressed",
        )
        .into());
    }
    println!("workgraph e2e fixture: workgraph service wired (ephemeral memory store)");

    // A small roster so the sidebar and owner labels have real identities.
    runtime
        .reconcile(vec![
            SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("planner")),
            SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("builder")),
        ])
        .await?;

    let decisions = build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "workgraph_e2e_dataset".to_string(),
            table: "workgraph_e2e_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy::default(),
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .map_err(|err| std::io::Error::other(format!("failed to build console decisions: {err:?}")))?;

    let listen_addr =
        std::env::var("META_WORKGRAPH_E2E_ADDR").unwrap_or_else(|_| "127.0.0.1:3240".to_string());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    let bound = listener.local_addr()?;
    println!("workgraph e2e fixture listening on http://{bound}");
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");

    let run_report = runtime
        .run(listener, decisions, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    run_report
        .shutdown
        .mob_stop
        .map_err(|err| std::io::Error::other(format!("failed to stop mob runtime: {err}")))?;
    run_report.serve_result?;
    Ok(())
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}
