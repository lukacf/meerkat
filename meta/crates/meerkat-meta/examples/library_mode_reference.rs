#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::TestClient;
// meerkat 0.7: the MeerkatId alias was deleted; member ids are AgentIdentity.
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConsolePolicy, DiscoverySpec, MetaConfig, PreSpawnData,
    RuntimeDecisionInputs, RuntimeOpsPolicy, SubscribeRequest, TrustedOidcRuntimeConfig,
    UnifiedRuntime, build_runtime_decision_state, handle_console_ingress_json,
};
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};

const MINIMAL_MOB_TOML: &str = r#"
[mob]
id = "reference-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition = MobDefinition::from_toml(MINIMAL_MOB_TOML)
        .map_err(|e| std::io::Error::other(format!("bad mob definition: {e}")))?;

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "reference-app".to_string(),
                    modules: vec![],
                },
                pre_spawn: Vec::<PreSpawnData>::new(),
            })
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await?;

    runtime.reconcile(reference_member_specs()).await?;
    runtime
        .subscribe_events(SubscribeRequest::default())
        .await?;

    let decisions = build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "reference_app_dataset".to_string(),
            table: "reference_app_table".to_string(),
        },
        trusted_meta_toml: trusted_modules_toml(),
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
    let _console_ingress_preview = handle_console_ingress_json(
        &decisions,
        r#"{"method":"GET","path":"/console/modules","auth":null}"#,
    );
    let _console_router = runtime.build_console_json_router(decisions.clone());

    let listen_addr =
        std::env::var("META_REF_ADDR").unwrap_or_else(|_| "127.0.0.1:3210".to_string());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    let bound = listener.local_addr()?;
    println!("reference app listening on http://{bound}");
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");
    println!("GET  /console");
    println!("GET  /console/experience");
    println!("GET  /console/modules");
    if std::env::var("META_REF_HTTP_MODE").ok().as_deref() == Some("serve") {
        runtime.serve(listener, decisions).await?;
        return Ok(());
    }

    let run_report = runtime
        .run(listener, decisions, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    let shutdown = run_report.shutdown;
    if shutdown.module_shutdown.orphan_processes != 0 {
        return Err(std::io::Error::other(format!(
            "runtime shutdown left {} orphan process(es)",
            shutdown.module_shutdown.orphan_processes
        ))
        .into());
    }
    shutdown
        .mob_stop
        .map_err(|err| std::io::Error::other(format!("failed to stop mob runtime: {err}")))?;
    run_report.serve_result?;
    Ok(())
}

fn reference_member_specs() -> Vec<SpawnMemberSpec> {
    use std::collections::BTreeMap;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    vec![
        // ── Coordinators ──
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("triage:main"))
            .with_labels(labels(&[
                ("display_name", "Triage"),
                ("group", "Coordinators"),
            ])),
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("router:main"))
            .with_labels(labels(&[
                ("display_name", "Router"),
                ("group", "Coordinators"),
            ])),
        // ── Domain agents ──
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("domain:billing"))
            .with_labels(labels(&[("display_name", "Billing"), ("group", "Domain")])),
        SpawnMemberSpec::new(
            ProfileName::from("lead"),
            MeerkatId::from("domain:delivery"),
        )
        .with_labels(labels(&[("display_name", "Delivery"), ("group", "Domain")])),
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("domain:support"))
            .with_labels(labels(&[("display_name", "Support"), ("group", "Domain")])),
        // ── Internal ──
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("gate:main")).with_labels(
            labels(&[
                ("display_name", "Gate"),
                ("group", "Internal"),
                ("addressable", "false"),
                ("singleton", "true"),
            ]),
        ),
        SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("monitor:health"))
            .with_labels(labels(&[
                ("display_name", "Health Monitor"),
                ("group", "Internal"),
                ("addressable", "false"),
                ("singleton", "true"),
            ])),
    ]
}

fn trusted_modules_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "memory"]
restart_policy = "on_failure"
"#
    .to_string()
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
