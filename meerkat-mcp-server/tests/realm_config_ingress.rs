//! Head realm config ingress on the rkat-mcp surface.
//!
//! `MeerkatMcpState::new_with_bootstrap_and_options` is the constructor
//! `rkat-mcp`'s `main` calls, so a construction error here is the process
//! refusing to start. The head realm document is authoritative: a head that
//! fails its ingress checks must stop startup with the typed `ConfigError`
//! text instead of being replaced by `Config::default()` and booting the
//! server on a configuration the operator never wrote.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::{ContextConfig, RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_mcp_server::MeerkatMcpState;
use std::path::Path;
use tempfile::TempDir;

/// The refused table, in the shape an operator hunting for a fleet-wide
/// Anthropic cache policy would write into `.rkat/config.toml`.
const HEAD_WITH_AGENT_PROVIDER_PARAMS: &str = "[agent]\n\
max_tokens_per_turn = 256\n\
provider_params = { provider_tag = { provider = \"anthropic\", cache_control = \"disabled\" } }\n";

/// The same head document without the refused table.
const HEAD_WITHOUT_AGENT_PROVIDER_PARAMS: &str = "[agent]\nmax_tokens_per_turn = 256\n";

/// Substring of `Config::reject_unwired_agent_provider_params`'s
/// `ConfigError::Validation` payload.
const REFUSAL_TEXT: &str = "[agent] provider_params is not applied to any session";

fn bootstrap(root: &Path, realm_id: &str) -> RuntimeBootstrap {
    let project_root = root.join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("project root should initialize");
    RuntimeBootstrap {
        realm: RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: realm_id.to_string(),
            },
            instance_id: Some("ingress".to_string()),
            backend_hint: Some("memory".to_string()),
            state_root: Some(root.join("realms")),
        },
        context: ContextConfig {
            context_root: Some(project_root),
            // Pin the user-global tail to an empty directory so the ambient
            // `~/.rkat/config.toml` never participates in this test.
            user_config_root: Some(root.join("user")),
        },
    }
}

fn write_head_config(root: &Path, realm_id: &str, body: &str) {
    let paths = meerkat_store::realm_paths_in(&root.join("realms"), realm_id);
    std::fs::create_dir_all(&paths.root).expect("realm dir should initialize");
    std::fs::write(&paths.config_path, body).expect("head config should be written");
}

#[tokio::test]
async fn head_config_with_agent_provider_params_refuses_mcp_startup() {
    let temp = TempDir::new().expect("temp dir");
    let realm_id = "mcp-realm-config-ingress-refused";
    write_head_config(temp.path(), realm_id, HEAD_WITH_AGENT_PROVIDER_PARAMS);

    let error = match MeerkatMcpState::new_with_bootstrap_and_test_client(
        bootstrap(temp.path(), realm_id),
        false,
    )
    .await
    {
        Ok(_state) => panic!(
            "rkat-mcp must refuse a head realm config carrying [agent] provider_params \
             instead of booting on Config::default()"
        ),
        Err(error) => error.to_string(),
    };

    assert!(
        error.contains(REFUSAL_TEXT),
        "startup error must carry the ingress refusal text; got: {error}"
    );
    assert!(
        error.contains("Validation error"),
        "startup error must surface the typed ConfigError::Validation; got: {error}"
    );
    assert!(
        error.contains(&format!("realm '{realm_id}'")),
        "startup error must name the realm whose head document was refused; got: {error}"
    );
}

#[tokio::test]
async fn head_config_without_agent_provider_params_starts_mcp() {
    let temp = TempDir::new().expect("temp dir");
    let realm_id = "mcp-realm-config-ingress-accepted";
    write_head_config(temp.path(), realm_id, HEAD_WITHOUT_AGENT_PROVIDER_PARAMS);

    let state = MeerkatMcpState::new_with_bootstrap_and_test_client(
        bootstrap(temp.path(), realm_id),
        false,
    )
    .await
    .expect("a head config without the refused table must start rkat-mcp");
    assert_eq!(state.realm_id().as_str(), realm_id);
}
