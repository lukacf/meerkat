//! Access Control Pack — runnable console server demonstrating the optional
//! ABAC layer end to end against the stock Meta console.
//!
//! Exposed as Cargo example `access_control_console`:
//!
//! ```bash
//! cargo run -p meerkat-meta --example access_control_console
//! ```
//!
//! The server bootstraps a tiny mob (one lead, two scouts with `org` labels),
//! wires an [`AccessController`] seeded from `access.toml`, and serves the
//! embedded console + REST/RPC/SSE surfaces. It runs the deterministic
//! [`TestClient`] so it needs **no API key** and gives stable replies.
//!
//! The console runs with `require_app_auth = false` (open console), so any
//! caller reaches it; callers that volunteer a valid bearer token are
//! identified and matched against the access rules, everyone else is
//! anonymous. The companion `persona-proxy.mjs` mints those tokens for four
//! personas so you can open one browser tab per identity.
//!
//! Seeded scenario (`access.toml`):
//! - `root@example.test` — admin: sees and does everything, gets the Access panel.
//! - `alice@example.test` — group `ops`: views every agent, may send only to `ops-lead`.
//! - `bob@example.test` — sees and sends to agents labelled `org=payments` only.
//! - anonymous — no grants: an empty console.
//!
//! Environment:
//! - `ACCESS_CONTROL_LISTEN_ADDR` — listen address (default `127.0.0.1:7300`).
//! - `ACCESS_CONTROL_WORK_DIR` — session + `access.toml` directory
//!   (default `<temp>/meta-access-control-pack`). Delete it to reset the
//!   seeded config; live console edits persist back into it.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
// meerkat 0.7: the MeerkatId alias was deleted; member ids are AgentIdentity.
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessGroup, AccessRule, AuthPolicy, BigQueryNaming,
    ConsolePolicy, DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime,
    build_runtime_decision_state,
};
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:7300";

/// `(profile, identity, &[(label_key, label_value)])` for a seeded member.
type SeedMember = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
    .to_string()
}

/// Local development OIDC: an HS256 issuer on a `.localhost` host, which the
/// runtime only trusts for development. `persona-proxy.mjs` mints tokens
/// against this issuer + shared secret.
fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "access_control_pack".to_string(),
            table: "access_control_pack".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "root@example.test".to_string(),
                "alice@example.test".to_string(),
                "bob@example.test".to_string(),
            ],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            // Open console: anyone reaches it; volunteered tokens identify
            // callers for per-user ABAC. Real deployments usually set this on.
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!(
            "../../crates/meerkat-meta/assets/release-targets.json"
        )
        .to_string(),
    })
    .expect("decision state builds")
}

/// The seeded scenario written to `access.toml` on first run.
fn seed_access_config() -> AccessControlConfig {
    AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        groups: BTreeMap::from([(
            "ops".to_string(),
            AccessGroup {
                description: Some("Operations".to_string()),
                members: vec!["alice@example.test".to_string()],
            },
        )]),
        rules: vec![
            AccessRule {
                id: "ops-view-all".to_string(),
                description: Some("Ops can see every agent".to_string()),
                groups: vec!["ops".to_string()],
                actions: vec!["agent.view".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "ops-send-lead".to_string(),
                description: Some("Ops can only message ops-lead".to_string()),
                groups: vec!["ops".to_string()],
                actions: vec!["agent.send".to_string()],
                agents: vec!["ops-lead".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "bob-payments-only".to_string(),
                description: Some("Bob is scoped to org=payments agents".to_string()),
                subjects: vec!["bob@example.test".to_string()],
                actions: vec!["agent.view".to_string(), "agent.send".to_string()],
                match_labels: BTreeMap::from([("org".to_string(), "payments".to_string())]),
                ..AccessRule::default()
            },
        ],
    }
}

fn main() {
    // Meerkat 0.7's generated machine-authority apply path allocates very
    // large stack frames in debug builds (see .cargo/config.toml). The
    // workspace-level `[env] RUST_MIN_STACK` only covers `cargo run`/test
    // flows, not direct execution of the prebuilt binary, so the example
    // sizes its own threads explicitly: the root future runs on a dedicated
    // 32 MiB thread and tokio workers get 32 MiB stacks (mirrors
    // meta_gateway/rpc_gateway's explicit worker sizing).
    const STACK_SIZE: usize = 32 * 1024 * 1024;
    std::thread::Builder::new()
        .name("access-control-runtime".into())
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(STACK_SIZE)
                .build()
                .expect("build tokio runtime");
            runtime.block_on(run());
        })
        .expect("spawn runtime thread")
        .join()
        .expect("runtime thread panicked");
}

async fn run() {
    let listen_addr =
        std::env::var("ACCESS_CONTROL_LISTEN_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.into());
    let work_dir = std::env::var("ACCESS_CONTROL_WORK_DIR").map_or_else(
        |_| std::env::temp_dir().join("meta-access-control-pack"),
        std::path::PathBuf::from,
    );
    std::fs::create_dir_all(&work_dir).expect("work dir");
    let session_path = work_dir.join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "access-control-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[profiles.scout]
model = "gpt-5.5"
external_addressable = true

[profiles.scout.tools]
comms = true
"#,
    )
    .expect("definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "access-control".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let mut runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(5))
        .await
        .expect("bootstrap runtime");

    // One lead plus two scouts; the `org` labels drive bob's label-selector rule.
    let members: &[SeedMember] = &[
        ("lead", "ops-lead", &[("org", "platform")]),
        ("scout", "scout-1", &[("org", "payments")]),
        ("scout", "scout-2", &[("org", "people")]),
    ];
    for (profile, identity, labels) in members {
        let mut spec = SpawnMemberSpec::from_wire(
            (*profile).to_string(),
            MeerkatId::from(*identity).to_string(),
            Some(format!("You are {identity}.").into()),
            None,
            None,
        );
        spec.labels = Some(
            labels
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        );
        runtime.spawn(spec).await.expect("spawn member");
    }

    // Seed access.toml on first run; live console edits persist back here.
    let access_path = work_dir.join("access.toml");
    if !access_path.exists() {
        std::fs::write(
            &access_path,
            toml::to_string_pretty(&seed_access_config()).expect("serialize access config"),
        )
        .expect("write access config");
    }
    let controller = AccessController::load(&access_path).expect("access controller");
    runtime.set_access_controller(controller);

    let app = runtime.build_reference_app_router(decision_state());
    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .unwrap_or_else(|err| panic!("bind {listen_addr}: {err}"));
    let bound = listener.local_addr().expect("local addr");
    println!("access control console listening on http://{bound}/console");
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");
    println!("access config persisted at {}", access_path.display());
    println!("mint persona tokens with persona-proxy.mjs to browse as each identity");
    axum::serve(listener, app).await.expect("serve");
}
