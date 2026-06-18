//! e2e-system: hierarchical realm config inheritance over a REAL filesystem +
//! REAL SQLite, exercising a three-level chain `app -> team -> global`.
//!
//! This is deterministic (no live LLM): it builds realm config documents on
//! disk, composes them through the production `FilesystemRealmConfigSource` +
//! `EffectiveConfigReader`, resolves connection targets through the real
//! connection resolver, and opens real SQLite-backed realm persistence to prove
//! state stays realm-local. The companion `system_shared_realm` suite covers the
//! cross-surface binary roundtrips; this suite pins the inheritance semantics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use meerkat_core::connection::{
    BindingId, GLOBAL_REALM_SLUG, RealmConfigSection, RealmId, WriteOwnerError,
    resolve_realm_binding_target_for_provider, resolve_write_owner,
};
use meerkat_core::{
    Config, ConfigStore, EffectiveConfigReader, FileConfigStore, Provider, RealmConfigSource,
};
use tempfile::TempDir;

/// Write a `Config` to `path` through the production `FileConfigStore` writer
/// (the same TOML writer `config set` uses), creating parents first.
async fn write_config_doc(path: &Path, config: &Config) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    FileConfigStore::new(path.to_path_buf(), meerkat_models::canonical())
        .set(config.clone())
        .await
        .expect("write realm config doc via FileConfigStore");
}

/// Write a hand-authored SPARSE realm config doc (raw TOML), creating parents.
/// Unlike [`write_config_doc`] (full serialization via `config set`), this leaves
/// unset keys ABSENT so the composer inherits them — the realistic style for a
/// hand-configured realm hierarchy.
async fn write_sparse_toml_doc(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(path, contents).await.unwrap();
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn realm_config_inheritance_real_fs_chain() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let state_root = temp.path().join("state");
    let global_doc = home.join(".rkat").join("config.toml");

    // --- global (chain tail): owns the anthropic binding + the base model and a
    // non-default top-level max_tokens. -------------------------------------
    let mut global = Config::default();
    global.agent.model = "claude-sonnet-4-5".to_string();
    global.max_tokens = Some(32_768);
    // Toggles set to the OPPOSITE of their struct default, so the child below
    // can override them BACK to the default — the case the `!= default` merge
    // collapses without presence (MF-6). `shell_enabled` default=false,
    // `schedule_enabled` default=true; `builtins_enabled` default=false is set
    // here and left unset by the child to prove pure toggle inheritance.
    global.tools.shell_enabled = true;
    global.tools.schedule_enabled = false;
    global.tools.builtins_enabled = true;
    // MF-8: a parent-realm MCP server that must survive into the child's
    // effective config (union by name, no tombstones).
    global.tools.mcp_servers = vec![meerkat_core::McpServerConfig::streamable_http(
        "global-mcp",
        "http://127.0.0.1:8888/mcp",
        std::collections::HashMap::new(),
    )];
    // MF-7: a parent disables a provider tool whose struct default is `true`; the
    // child below must be able to re-enable it (presence override-to-default).
    global.provider_tools.anthropic.web_search = false;
    global.realm.insert(
        GLOBAL_REALM_SLUG.to_string(),
        RealmConfigSection::from_inline_api_keys(&[("anthropic", "sk-ant-test-global")]),
    );
    write_config_doc(&global_doc, &global).await;

    // --- team (mid): inherits global's binding + model, overrides max_tokens
    // DOWN to a non-default value. Proves a parent-chain override of a scalar. -
    let team_paths = meerkat_store::realm_paths_in(&state_root, "team");
    write_sparse_toml_doc(
        &team_paths.config_path,
        "max_tokens = 16384\n\n[realm.team]\nparent = \"global\"\n",
    )
    .await;

    // --- app (head): inherits team's max_tokens + global's binding, overrides
    // the model. ------------------------------------------------------------
    let app_paths = meerkat_store::realm_paths_in(&state_root, "app");
    // app is a SPARSE hand-authored doc: it sets ONLY the keys it overrides and
    // OMITS the rest. It overrides shell_enabled/schedule_enabled to values
    // EQUAL to their struct defaults (the case a `!= default` merge collapses —
    // presence-aware compose must honor the explicit key) and never mentions
    // builtins_enabled (so it inherits global's non-default true).
    write_sparse_toml_doc(
        &app_paths.config_path,
        "[realm.app]\n\
         parent = \"team\"\n\n\
         [agent]\n\
         model = \"claude-opus-4-8\"\n\n\
         [tools]\n\
         shell_enabled = false\n\
         schedule_enabled = true\n\n\
         [[tools.mcp_servers]]\n\
         name = \"app-mcp\"\n\
         url = \"http://127.0.0.1:9999/mcp\"\n\n\
         [provider_tools.anthropic]\n\
         web_search = true\n",
    )
    .await;

    // --- compose the effective config for `app` over the REAL fs source. -----
    let source: Arc<dyn RealmConfigSource> =
        Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            state_root.clone(),
            global_doc.clone(),
            meerkat_models::canonical(),
        ));
    let app_realm = RealmId::parse("app").unwrap();
    let effective = EffectiveConfigReader::new(Arc::clone(&source))
        .effective_config(&app_realm)
        .await
        .expect("app effective config composes over the real fs chain");

    // Model: app's own override wins over the inherited global base.
    assert_eq!(
        effective.agent.model, "claude-opus-4-8",
        "head realm overrides the inherited model"
    );
    // max_tokens: app is unset (None) -> inherits team's explicit override
    // (16_384), NOT global's 32_768. This is the presence-based merge: an unset
    // child inherits, and team's override of global is honored down the chain.
    assert_eq!(
        effective.max_tokens,
        Some(16_384),
        "team's explicit max_tokens override is inherited by app"
    );
    assert_eq!(effective.resolved_max_tokens(), 16_384);

    // Toggle override (MF-6): the child explicitly set each toggle to a value
    // EQUAL to its struct default; presence-aware compose honors the explicit
    // key so the parent's opposite value does not leak through.
    assert!(
        !effective.tools.shell_enabled,
        "child shell_enabled=false (the default) overrides parent's true"
    );
    assert!(
        effective.tools.schedule_enabled,
        "child schedule_enabled=true (the default) overrides parent's false"
    );
    // Pure inheritance: the child never set builtins_enabled, so it inherits
    // global's non-default true.
    assert!(
        effective.tools.builtins_enabled,
        "unset child inherits global's builtins_enabled=true"
    );

    // MF-8: MCP servers union by name across the chain — the inherited parent
    // server must NOT be clobbered when the child adds its own.
    let mcp_names: Vec<&str> = effective
        .tools
        .mcp_servers
        .iter()
        .map(|server| server.name.as_str())
        .collect();
    assert!(
        mcp_names.contains(&"global-mcp"),
        "inherited parent MCP server survives (no clobber): {mcp_names:?}"
    );
    assert!(
        mcp_names.contains(&"app-mcp"),
        "child MCP server is added (union): {mcp_names:?}"
    );

    // MF-7: the child re-enables a provider web_search the parent disabled, even
    // though `true` is the struct default (presence override-to-default for
    // provider_tools).
    assert!(
        effective.provider_tools.anthropic.web_search,
        "child re-enables an inherited disabled provider web_search"
    );

    // The chain's realm sections are all present in the composed config.
    for realm in ["app", "team", GLOBAL_REALM_SLUG] {
        assert!(
            effective.realm.contains_key(realm),
            "composed config carries the '{realm}' section"
        );
    }

    // --- binding resolution: app has no binding of its own; resolution walks
    // app -> team -> global and resolves global's anthropic binding, stamped
    // with the OWNING realm (global), not the requesting realm (app). ---------
    let target = resolve_realm_binding_target_for_provider(
        &effective,
        Provider::Anthropic,
        Some(&app_realm), // explicit head realm
        None,             // explicit_binding
        None,             // explicit_profile
        None,             // preferred_realm
        false,            // allow_env_default — the inherited section must resolve on its own
    )
    .expect("app resolves the inherited anthropic binding through the chain");
    assert_eq!(
        target.auth_binding.realm.as_str(),
        GLOBAL_REALM_SLUG,
        "owning-realm provenance: the resolved binding belongs to global, not app"
    );

    // --- strict-owner write: a write for app's INHERITED binding is REJECTED
    // (app may not shadow global's binding into its own doc); the policy
    // identifies the owning realm so the caller can retry with `--realm global`.
    let binding: BindingId = target.auth_binding.binding.clone();
    let reject = resolve_write_owner(&effective, &app_realm, &binding)
        .expect_err("writing an inherited binding from the requesting realm is rejected");
    match reject {
        WriteOwnerError::Inherited { owner, .. } => assert_eq!(
            owner.as_str(),
            GLOBAL_REALM_SLUG,
            "strict-owner rejection identifies global as the owning realm"
        ),
        other => panic!("expected WriteOwnerError::Inherited, got {other:?}"),
    }
    // The OWNING realm (global) may write its own binding.
    let global_realm = RealmId::parse(GLOBAL_REALM_SLUG).unwrap();
    let owner_ok = resolve_write_owner(&effective, &global_realm, &binding)
        .expect("the owning realm can write its own binding");
    assert_eq!(
        owner_ok.as_str(),
        GLOBAL_REALM_SLUG,
        "the realm that owns a binding is its write owner"
    );

    // --- state stays realm-local: real SQLite stores live under each realm's
    // own root, never shared. -------------------------------------------------
    for realm in ["app", "team"] {
        let (_manifest, _persistence) = meerkat::open_realm_persistence_in(
            &state_root,
            realm,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("realm persistence opens with a real sqlite backend");
    }
    let app_sqlite = meerkat_store::realm_paths_in(&state_root, "app").sessions_sqlite_path;
    let team_sqlite = meerkat_store::realm_paths_in(&state_root, "team").sessions_sqlite_path;
    assert_ne!(
        app_sqlite, team_sqlite,
        "each realm gets its own sqlite store path (state is realm-local)"
    );
    assert!(
        app_sqlite.starts_with(&app_paths.root),
        "app's sqlite store lives under app's realm root"
    );

    // --- inherited-but-unset child still resolves global's binding directly
    // (2-level: a fresh realm with no parent edge uses the implicit global tail).
    let solo_paths = meerkat_store::realm_paths_in(&state_root, "solo");
    let mut solo = Config::default();
    solo.realm
        .insert("solo".to_string(), RealmConfigSection::default());
    write_config_doc(&solo_paths.config_path, &solo).await;
    let solo_realm = RealmId::parse("solo").unwrap();
    let solo_effective = EffectiveConfigReader::new(source)
        .effective_config(&solo_realm)
        .await
        .expect("solo composes against the implicit global tail");
    let solo_target = resolve_realm_binding_target_for_provider(
        &solo_effective,
        Provider::Anthropic,
        Some(&solo_realm),
        None,
        None,
        None,
        false,
    )
    .expect("a realm with no parent edge still inherits the global tail binding");
    assert_eq!(
        solo_target.auth_binding.realm.as_str(),
        GLOBAL_REALM_SLUG,
        "the implicit global tail is always part of the chain"
    );

    // MF-13: an operator default model set only in `global` ([agent].model) is
    // inherited by a realm that does not override it, so the composed config the
    // create-session default-model ladder reads carries the inherited default
    // (not the catalog substitute). `solo` never sets a model.
    assert_eq!(
        solo_effective.agent.model, "claude-sonnet-4-5",
        "solo inherits global's operator default model"
    );
}
