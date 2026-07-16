use std::fs;
use std::path::Path;

fn read_manifest(path: impl AsRef<Path>) -> Result<toml::Value, String> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    toml::from_str(&text).map_err(|err| format!("parse {}: {err}", path.display()))
}

fn string_array<'a>(value: &'a toml::Value, path: &str) -> Result<Vec<&'a str>, String> {
    value
        .get("features")
        .and_then(|features| features.get(path))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("manifest missing features.{path} array"))?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .ok_or_else(|| format!("features.{path} contains non-string entry"))
        })
        .collect()
}

fn dependency_table<'a>(
    manifest: &'a toml::Value,
    name: &str,
) -> Result<&'a toml::value::Table, String> {
    manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get(name))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("manifest should depend on {name}"))
}

fn workspace_dependency_table<'a>(
    manifest: &'a toml::Value,
    name: &str,
) -> Result<&'a toml::value::Table, String> {
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get(name))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("workspace should depend on {name}"))
}

fn dependency_features<'a>(manifest: &'a toml::Value, name: &str) -> Result<Vec<&'a str>, String> {
    Ok(dependency_table(manifest, name)?
        .get("features")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect())
}

#[test]
fn slim_cli_does_not_enable_openai_realtime_unconditionally() -> Result<(), String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cli_manifest = read_manifest(manifest_dir.join("Cargo.toml"))?;

    let default_features = string_array(&cli_manifest, "default")?;
    assert!(
        default_features.contains(&"openai-realtime"),
        "default rkat builds should keep OpenAI realtime support"
    );

    let realtime_features = string_array(&cli_manifest, "openai-realtime")?;
    assert_eq!(
        realtime_features,
        vec![
            "openai",
            "meerkat/openai-realtime",
            "meerkat/live",
            "dep:meerkat-live",
            "meerkat-rpc?/openai-realtime"
        ],
        "the CLI realtime feature is explicit; `rkat mob host` composes the live \
         plane (LiveAdapterHost) under it per multi-host §16.5/§21.4, so slim \
         builds that drop openai-realtime also drop meerkat-live"
    );

    let meerkat_dependency = dependency_table(&cli_manifest, "meerkat")?;
    let unconditional_features = meerkat_dependency
        .get("features")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        !unconditional_features.contains(&"openai-realtime"),
        "rkat --no-default-features must not pull the OpenAI realtime stack through its facade dependency"
    );
    Ok(())
}

#[test]
fn integration_tests_restore_rpc_default_feature_surface() -> Result<(), String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .ok_or_else(|| "rkat manifest dir should have workspace parent".to_string())?;
    let integration_manifest = read_manifest(workspace_root.join("tests/integration/Cargo.toml"))?;

    let rpc_dependency = dependency_table(&integration_manifest, "meerkat-rpc")?;
    assert_eq!(
        rpc_dependency
            .get("workspace")
            .and_then(toml::Value::as_bool),
        Some(true),
        "integration tests should still inherit the workspace meerkat-rpc path/version"
    );

    let features = dependency_features(&integration_manifest, "meerkat-rpc")?;
    for expected in [
        "comms",
        "mcp",
        "mob",
        "openai-realtime",
        "schedule",
        "workgraph",
    ] {
        assert!(
            features.contains(&expected),
            "integration tests should opt back into meerkat-rpc/{expected} after workspace defaults are suppressed"
        );
    }
    Ok(())
}

#[test]
fn slim_cli_mob_feature_does_not_inherit_rpc_realtime_defaults() -> Result<(), String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .ok_or_else(|| "rkat manifest dir should have workspace parent".to_string())?;
    let workspace_manifest = read_manifest(workspace_root.join("Cargo.toml"))?;
    let cli_manifest = read_manifest(manifest_dir.join("Cargo.toml"))?;
    let rpc_manifest = read_manifest(workspace_root.join("meerkat-rpc").join("Cargo.toml"))?;

    let rpc_dependency = workspace_dependency_table(&workspace_manifest, "meerkat-rpc")?;
    assert_eq!(
        rpc_dependency
            .get("default-features")
            .and_then(toml::Value::as_bool),
        Some(false),
        "rkat's optional RPC dependency must not enable RPC defaults in slim feature builds"
    );

    let mob_features = string_array(&cli_manifest, "mob")?;
    assert!(
        !mob_features
            .iter()
            .any(|feature| feature.contains("meerkat-rpc")),
        "the slim CLI mob feature should not enable the hosted RPC surface"
    );
    assert!(
        mob_features.contains(&"session-store"),
        "mob commands use persistent sessions, so slim mob builds should request that feature explicitly"
    );

    let default_features = string_array(&cli_manifest, "default")?;
    assert!(
        default_features.contains(&"rpc-surface"),
        "default rkat builds should keep the hosted RPC surface"
    );

    let rpc_surface_features = string_array(&cli_manifest, "rpc-surface")?;
    assert!(
        rpc_surface_features.contains(&"dep:meerkat-rpc")
            && rpc_surface_features.contains(&"meerkat-rpc/mob"),
        "the RPC surface feature should own the optional meerkat-rpc dependency"
    );

    let rpc_default_features = string_array(&rpc_manifest, "default")?;
    assert!(
        rpc_default_features.contains(&"openai-realtime"),
        "default RPC builds should preserve OpenAI realtime support"
    );

    let rpc_realtime_features = string_array(&rpc_manifest, "openai-realtime")?;
    assert_eq!(
        rpc_realtime_features,
        vec!["meerkat/openai-realtime", "meerkat-client/openai-realtime"],
        "RPC realtime support should be explicit so slim rkat mob builds do not inherit it"
    );

    for dependency in ["meerkat", "meerkat-client"] {
        let features = dependency_table(&rpc_manifest, dependency)?
            .get("features")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>();
        assert!(
            !features.contains(&"openai-realtime"),
            "meerkat-rpc should not hard-enable openai-realtime on its {dependency} dependency"
        );
    }
    Ok(())
}

#[test]
fn facade_does_not_pull_websocket_transport_without_live_features() -> Result<(), String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .ok_or_else(|| "rkat manifest dir should have workspace parent".to_string())?;
    let facade_manifest = read_manifest(workspace_root.join("meerkat").join("Cargo.toml"))?;

    let native_dependencies = facade_manifest
        .get("target")
        .and_then(|target| target.get("cfg(not(target_arch = \"wasm32\"))"))
        .and_then(|target| target.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "meerkat facade should declare native target dependencies".to_string())?;
    assert!(
        !native_dependencies.contains_key("tokio-tungstenite"),
        "WebSocket transport belongs behind live/realtime features, not the slim facade graph"
    );
    Ok(())
}
