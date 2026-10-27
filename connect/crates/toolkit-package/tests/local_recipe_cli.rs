#![cfg(feature = "local-recipes")]

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use toolkit_package::{
    ActivationManifest, ContentDigest, IntegrationBundle, RecipePlan, canonical_json,
};

const PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const MCP: &[u8] = include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");

struct Fixture {
    _temp: TempDir,
    database: PathBuf,
    plugin: PathBuf,
    mcp: PathBuf,
    recipes: PathBuf,
}

fn path(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let plugin = root.join("plugin.json");
        let mut value: Value = serde_json::from_slice(PLUGIN).unwrap();
        value["extensions"]["ai.rkat.toolkit"]["identity"]["publisher"] = json!("local-research");
        value["extensions"]["ai.rkat.toolkit"]["identity"]["name"] = json!("selected-tracker");
        fs::write(&plugin, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let mcp = root.join("mcp.json");
        fs::write(&mcp, MCP).unwrap();
        let recipes = root.join("recipes.json");
        fs::write(&recipes, RECIPES).unwrap();
        Self {
            database: root.join("recipes.sqlite"),
            _temp: temp,
            plugin,
            mcp,
            recipes,
        }
    }

    fn args(&self, verb: &str, realm: &str, owner: &str) -> Vec<String> {
        vec![
            verb.into(),
            "--database".into(),
            path(&self.database),
            "--realm".into(),
            realm.into(),
            "--owner".into(),
            owner.into(),
        ]
    }

    fn save(&self) -> Vec<String> {
        let mut args = self.args("save", "host-realm", "alice");
        args.extend([
            "--plugin".into(),
            path(&self.plugin),
            "--mcp".into(),
            path(&self.mcp),
            "--recipes".into(),
            path(&self.recipes),
            "--decision-owner".into(),
            "setup-decisions".into(),
            "--decision-record".into(),
            "tracker-choice".into(),
            "--decision-revision".into(),
            "1".into(),
        ]);
        args
    }

    fn read(&self, realm: &str, owner: &str, digest: &str) -> Vec<String> {
        let mut args = self.args("read", realm, owner);
        args.extend([
            "--publisher".into(),
            "local-research".into(),
            "--name".into(),
            "selected-tracker".into(),
            "--version".into(),
            "0.1.0".into(),
            "--digest".into(),
            digest.into(),
        ]);
        args
    }

    fn bundle(&self) -> IntegrationBundle {
        IntegrationBundle::parse(
            &fs::read(&self.plugin).unwrap(),
            Some(&fs::read(&self.mcp).unwrap()),
            &fs::read(&self.recipes).unwrap(),
        )
        .unwrap()
    }

    fn selection(&self, recipe_index: usize) -> Value {
        let bundle = self.bundle();
        let recipe = &bundle.recipes().recipes[recipe_index];
        let RecipePlan::RemoteMcp {
            server, publisher, ..
        } = &recipe.plan
        else {
            panic!("remote fixture")
        };
        let toolkit_package::PortableMcpServer::StreamableHttp { url, .. } =
            &bundle.servers()[server]
        else {
            panic!("HTTP fixture")
        };
        let observed = ContentDigest::of_bytes(b"offline fixture observation");
        // Concrete test declarations only. Export must not turn these references
        // or invented operation schemas into authentication or readiness proof.
        json!({
            "schema_version": 1, "package": bundle.package().identity,
            "package_content_digest": bundle.package().content_digest().unwrap(),
            "strategy": recipe.strategy,
            "configuration": {"owner": "fixture/config", "reference": "chosen", "revision": "1", "content_digest": observed},
            "policy": {"owner": "fixture/policy", "revision": "1", "content_digest": observed},
            "components": [{
                "id": "remote", "version": "offline-observed-contract",
                "source": {"kind": "remote_service", "endpoint": url, "publisher": publisher,
                    "configuration_revision": "1", "observed_contract": observed, "transport": "mcp_streamable_http"},
                "runtime": "remote", "placement": "remote", "lifetime": "externally_managed",
                "activation_scope": {"kind": "independent"},
                "dependencies": [], "required_contracts": [], "host_services": [], "state": [],
                "readiness": {"owner": "fixture/mcp", "checks": ["observe-current-tools"]},
                "shutdown": {"owner": "fixture/mcp", "operation": "disconnect", "maximum_wait_ms": 1000, "timeout": "preserve_uncertainty"},
                "rollback": {"kind": "owner_reconciliation", "owner": "fixture/mcp", "operation": "reconcile"}
            }],
            "exports": [{
                "role": "service_connector", "id": recipe.export, "components": ["remote"],
                "authentication": {"kind": "binding", "owner": "fixture/auth", "requirement": "account", "credential_audience": publisher},
                "operations": [{"id": "fixture-read", "schema_digest": observed, "declared_effect": "read",
                    "required_authority": ["read"], "resources": {"kind": "connection_wide"},
                    "result_limits": {"maximum_bytes": 4096, "maximum_items": 10}}],
                "events": [], "enforcement_contract": {"name": "fixture/policy", "version": {"major": 1, "minimum_minor": 0}},
                "diagnostics": "check-account", "cleanup": "release-binding"
            }],
            "required_contracts": [], "host_services": [],
            "portable_mcp_entries": [{"name": server, "component": "remote"}]
        })
    }

    fn export(&self, digest: &str, selection: &Path, destination: &Path) -> Vec<String> {
        let mut args = self.read("host-realm", "alice", digest);
        args[0] = "export".into();
        args.extend([
            "--selection".into(),
            path(selection),
            "--destination".into(),
            path(destination),
        ]);
        args
    }

    fn check_export(&self, digest: &str, selection: &Path, destination: &Path) -> Vec<String> {
        let mut args = self.export(digest, selection, destination);
        args[0] = "check-export".into();
        args
    }
}

fn invoke(args: &[String]) -> Output {
    // Every operation gets a new process, no caller configuration or secrets.
    Command::new(env!("CARGO_BIN_EXE_toolkit-local-recipes"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap()
}

fn success(args: &[String]) -> Value {
    let output = invoke(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["scope"], "retained_recipe_metadata");
    value
}

fn failure(args: &[String], kind: &str) -> Output {
    let output = invoke(args);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut expected = json!({
        "schema_version": 1, "scope": "retained_recipe_metadata",
        "status": "error", "error": kind,
    });
    if kind == "invalid_bundle" {
        assert_eq!(value["diagnostic"]["phase"], "before_store_open");
        expected["diagnostic"] = value["diagnostic"].clone();
    }
    assert_eq!(value, expected);
    output
}

#[test]
fn exact_choice_reopens_replays_and_stays_in_its_operator_scope() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    assert_eq!(saved["status"], "retained");
    assert_eq!(
        saved["recipe"]["origin"],
        json!({
            "kind": "local_investigation",
            "decision": {"owner": "setup-decisions", "record": "tracker-choice", "revision": "1"},
        })
    );
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let read = success(&fixture.read("host-realm", "alice", digest));
    assert_eq!(read["status"], "found");
    assert_eq!(
        read["recipe"]["documents"],
        json!({
            "plugin_json": fs::read_to_string(&fixture.plugin).unwrap(),
            "mcp_json": fs::read_to_string(&fixture.mcp).unwrap(),
            "recipes_json": fs::read_to_string(&fixture.recipes).unwrap(),
        })
    );
    assert_eq!(success(&fixture.save()), saved);
    let listed = success(&fixture.args("list", "host-realm", "alice"));
    assert_eq!(listed["recipes"], json!([saved["recipe"].clone()]));
    assert_eq!(listed["unverified"], json!([]));
    for (realm, owner) in [("host-realm", "bob"), ("other-realm", "alice")] {
        assert_eq!(
            success(&fixture.args("list", realm, owner))["recipes"],
            json!([])
        );
        assert_eq!(
            success(&fixture.read(realm, owner, digest))["status"],
            "not_found"
        );
    }
}

#[test]
fn changed_documents_or_decision_conflict_and_wrong_digest_never_returns_documents() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let original = success(&fixture.read("host-realm", "alice", digest));
    let mut changed_decision = fixture.save();
    *changed_decision.last_mut().unwrap() = "2".into();
    failure(&changed_decision, "immutable_conflict");
    // Even whitespace is an exact-document change under the saved identity.
    let mut bytes = fs::read(&fixture.plugin).unwrap();
    bytes.push(b'\n');
    fs::write(&fixture.plugin, bytes).unwrap();
    failure(&fixture.save(), "immutable_conflict");
    assert_eq!(
        success(&fixture.read("host-realm", "alice", digest)),
        original
    );
    let wrong = if digest == "0".repeat(64) {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    failure(
        &fixture.read("host-realm", "alice", &wrong),
        "stored_recipe_unverified",
    );
}

#[test]
fn observing_a_missing_or_uninitialized_database_cannot_create_a_store() {
    let fixture = Fixture::new();
    failure(
        &fixture.args("list", "host-realm", "alice"),
        "database_not_found",
    );
    failure(
        &fixture.read("host-realm", "alice", &"0".repeat(64)),
        "database_not_found",
    );
    assert!(!fixture.database.exists());
    fs::write(&fixture.database, b"").unwrap();
    failure(
        &fixture.args("list", "host-realm", "alice"),
        "database_unavailable",
    );
    // SQLite may touch a header or WAL; it must not initialize Toolkit tables.
    let db = rusqlite::Connection::open(&fixture.database).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'toolkit_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn strict_arguments_and_invalid_documents_fail_before_database_creation_without_value_echo() {
    let fixture = Fixture::new();
    let canary = "PRIVATE_VALUE_MUST_NOT_APPEAR";
    let mut args = fixture.save();
    args.extend(["--owner".into(), canary.into()]);
    failure(&args, "invalid_arguments");
    let mut args = fixture.save();
    args.extend(["--token".into(), canary.into()]);
    failure(&args, "invalid_arguments");
    let mut args = fixture.save();
    let index = args.iter().position(|x| x == "--plugin").unwrap() + 1;
    args[index] = format!("{canary}.json");
    failure(&args, "absolute_path_required");
    let mut args = fixture.save();
    let index = args.iter().position(|x| x == "--decision-record").unwrap() + 1;
    args[index] = format!("{canary} invalid");
    failure(&args, "invalid_decision");
    fs::write(&fixture.plugin, format!("{{bad-json:{canary}}}")).unwrap();
    let output = failure(&fixture.save(), "invalid_bundle");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(canary));
    assert!(!fixture.database.exists());
}

#[test]
fn oversized_special_files_and_inline_auth_are_rejected_before_save() {
    let fixture = Fixture::new();
    fs::write(&fixture.plugin, vec![b' '; 1024 * 1024 + 1]).unwrap();
    failure(&fixture.save(), "input_too_large");
    fs::remove_file(&fixture.plugin).unwrap();
    fs::create_dir(&fixture.plugin).unwrap();
    failure(&fixture.save(), "regular_file_required");
    fs::remove_dir(&fixture.plugin).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&fixture.recipes, &fixture.plugin).unwrap();
        failure(&fixture.save(), "regular_file_required");
        fs::remove_file(&fixture.plugin).unwrap();
    }
    fs::write(&fixture.plugin, PLUGIN).unwrap();
    let mut mcp: Value = serde_json::from_slice(MCP).unwrap();
    mcp["mcpServers"]["github-readonly"]["headers"] = json!({
        "Authorization": "PRIVATE_VALUE_MUST_NOT_APPEAR",
    });
    fs::write(&fixture.mcp, serde_json::to_vec(&mcp).unwrap()).unwrap();
    failure(&fixture.save(), "invalid_bundle");
    assert!(!fixture.database.exists());
}

#[test]
fn guidance_without_mcp_is_retained_with_blockers_and_no_activation_claim() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.plugin,
        include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/plugin.json"),
    )
    .unwrap();
    fs::write(
        &fixture.recipes,
        include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/recipes.json"),
    )
    .unwrap();
    let mut args = fixture.save();
    let index = args.iter().position(|x| x == "--mcp").unwrap();
    args.drain(index..index + 2);
    let saved = success(&args);
    assert_eq!(saved["status"], "retained");
    let identity = &saved["recipe"]["identity"];
    let mut read = fixture.args("read", "host-realm", "alice");
    for field in ["publisher", "name", "version"] {
        read.extend([
            format!("--{field}"),
            identity[field].as_str().unwrap().into(),
        ]);
    }
    read.extend([
        "--digest".into(),
        saved["recipe"]["digest"].as_str().unwrap().into(),
    ]);
    let loaded = success(&read);
    assert!(loaded["recipe"]["documents"]["mcp_json"].is_null());
    let recipes: Value = serde_json::from_str(
        loaded["recipe"]["documents"]["recipes_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recipes["recipes"][0]["plan"]["kind"], "guidance_only");
    assert!(
        !recipes["recipes"][0]["plan"]["blockers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn explicitly_unauthenticated_recipe_survives_save_reopen_and_portable_export() {
    let fixture = Fixture::new();
    let mut plugin: Value = serde_json::from_slice(&fs::read(&fixture.plugin).unwrap()).unwrap();
    let mut recipes: Value = serde_json::from_slice(RECIPES).unwrap();
    let mut mcp: Value = serde_json::from_slice(MCP).unwrap();
    let source_url = "https://example.invalid/public-mcp-documentation";
    plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0]["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!(source_url));
    // Synthetic provider evidence, not a claim about the GitHub fixture's service.
    recipes["sources"].as_array_mut().unwrap().push(json!({
        "id": "public-fixture", "url": source_url,
        "observed_sha256": ContentDigest::of_bytes(b"synthetic public endpoint documentation"),
        "upstream": {"kind": "documentation_observation"}, "license": "Synthetic test data"
    }));
    recipes["recipes"][0]["prerequisites"] = json!([{
        "kind": "application_binding", "instruction": "The host must admit this endpoint."
    }]);
    recipes["recipes"][0]["plan"]["authentication"] = json!({
        "kind": "unauthenticated", "evidence": "public-fixture"
    });
    recipes["recipes"][0]["plan"]["publisher"] = json!("fixture-public-provider");
    recipes["recipes"][0]["plan"]["read_only_evidence"] = json!("public-fixture");
    mcp["mcpServers"]["github-readonly"]["url"] = json!("https://example.invalid/public-mcp");
    for (path, value) in [
        (&fixture.plugin, &plugin),
        (&fixture.recipes, &recipes),
        (&fixture.mcp, &mcp),
    ] {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let reopened = success(&fixture.read("host-realm", "alice", digest));
    assert_eq!(
        reopened["recipe"]["documents"]["recipes_json"],
        fs::read_to_string(&fixture.recipes).unwrap()
    );
    assert_eq!(success(&fixture.save()), saved);
    let mut selected = fixture.selection(0);
    let selection_path = fixture._temp.path().join("public-selection.json");
    let destination = fixture._temp.path().join("public-export");
    fs::write(
        &selection_path,
        serde_json::to_vec_pretty(&selected).unwrap(),
    )
    .unwrap();
    failure(
        &fixture.export(digest, &selection_path, &destination),
        "portable_export_refused",
    );
    assert!(
        !destination.exists(),
        "a mismatched binding must not create an export"
    );
    selected["exports"][0]["authentication"] = json!({"kind": "none"});
    fs::write(
        &selection_path,
        serde_json::to_vec_pretty(&selected).unwrap(),
    )
    .unwrap();
    let exported = success(&fixture.export(digest, &selection_path, &destination));
    assert_eq!(exported["status"], "exported");
    assert_eq!(exported["recipe"], saved["recipe"]);
    assert!(exported.get("declared_remote_service").is_none());
    let legacy_check = success(&fixture.check_export(digest, &selection_path, &destination));
    assert!(legacy_check.get("declared_remote_service").is_none());
    assert_eq!(legacy_check["export_check"]["state"], "matches");

    // Exact declared raw names need not equal logical operation IDs. This is
    // synthetic source evidence, not an observation of the hosted service.
    selected["portable_mcp_entries"][0]["operation_tools"] = json!({"fixture-read":"actual.lookup"});
    fs::write(&selection_path, serde_json::to_vec_pretty(&selected).unwrap()).unwrap();
    let declared_destination = fixture._temp.path().join("public-declared-export");
    let declared = success(&fixture.export(digest, &selection_path, &declared_destination));
    let expected = json!({
        "manifest_digest": declared["selection_digest"],
        "component":"remote", "export":selected["exports"][0]["id"],
        "mcp_name":"github-readonly", "authentication":{"kind":"none"},
        "operation_tools":{"fixture-read":"actual.lookup"},
    });
    assert_eq!(declared["declared_remote_service"], expected);
    let checked = success(&fixture.check_export(digest, &selection_path, &declared_destination));
    assert_eq!(checked["declared_remote_service"], expected);
    assert_eq!(checked["export_check"]["state"], "matches");
    for name in ["plugin.json", "mcp.json"] {
        assert_eq!(fs::read(destination.join(name)).unwrap(), fs::read(declared_destination.join(name)).unwrap());
    }
    // Metadata is returned even when files are missing, but cannot be mistaken
    // for an exported configuration or connection readiness.
    let absent = success(&fixture.check_export(digest, &selection_path, &fixture._temp.path().join("absent")));
    assert_eq!(absent["declared_remote_service"], expected);
    assert_eq!(absent["export_check"]["state"], "missing");
    let actual: Value =
        serde_json::from_slice(&fs::read(destination.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(
        actual["mcpServers"]["github-readonly"]["url"],
        "https://example.invalid/public-mcp"
    );
    assert!(
        actual["mcpServers"]["github-readonly"]
            .get("headers")
            .is_none()
    );
    assert_eq!(
        success(&fixture.read("host-realm", "alice", digest)),
        reopened
    );
}

#[test]
fn remote_declared_service_uses_canonical_selection_for_export_and_readback() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let mut selected = fixture.selection(0);
    selected["portable_mcp_entries"][0]["operation_tools"] = json!({"fixture-read":"provider.read"});
    let path = fixture._temp.path().join("selected.json");
    fs::write(&path, serde_json::to_vec(&selected).unwrap()).unwrap();
    let destination = fixture._temp.path().join("binding-export");
    let exported = success(&fixture.export(digest, &path, &destination));
    let checked = success(&fixture.check_export(digest, &path, &destination));
    assert_eq!(exported["declared_remote_service"], checked["declared_remote_service"]);
    assert_eq!(exported["declared_remote_service"]["authentication"], selected["exports"][0]["authentication"]);
    assert_eq!(exported["declared_remote_service"]["authentication"]["kind"], "binding");
    assert_eq!(exported["declared_remote_service"]["operation_tools"], json!({"fixture-read":"provider.read"}));
    let original = fs::read(destination.join("mcp.json")).unwrap();
    for mode in ["empty", "wrong-id", "extra-id", "bad-name", "duplicate-name", "wrong-auth", "wrong-server"] {
        let mut changed = selected.clone();
        match mode {
            "empty" => changed["portable_mcp_entries"][0]["operation_tools"] = json!({}),
            "wrong-id" => changed["portable_mcp_entries"][0]["operation_tools"] = json!({"other":"provider.read"}),
            "extra-id" => changed["portable_mcp_entries"][0]["operation_tools"]["other"] = json!("provider.other"),
            "bad-name" => changed["portable_mcp_entries"][0]["operation_tools"]["fixture-read"] = json!("not a raw name"),
            "duplicate-name" => {
                let mut second = changed["exports"][0]["operations"][0].clone();
                second["id"] = json!("other");
                changed["exports"][0]["operations"].as_array_mut().unwrap().push(second);
                changed["portable_mcp_entries"][0]["operation_tools"]["other"] = json!("provider.read");
            }
            "wrong-auth" => changed["exports"][0]["authentication"] = json!({"kind":"none"}),
            "wrong-server" => changed["portable_mcp_entries"][0]["name"] = json!("other-server"),
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let refused = fixture._temp.path().join(format!("refused-{mode}"));
        failure(&fixture.export(digest, &path, &refused), "portable_export_refused");
        assert!(!refused.exists());
        failure(&fixture.check_export(digest, &path, &destination), "portable_export_refused");
        assert_eq!(fs::read(destination.join("mcp.json")).unwrap(), original);
    }
}

#[test]
fn export_emits_only_the_selected_strategy_with_exact_hashes_and_retained_origin() {
    let fixture = Fixture::new();
    let mut plugin: Value = serde_json::from_slice(&fs::read(&fixture.plugin).unwrap()).unwrap();
    let mut recipes: Value = serde_json::from_slice(RECIPES).unwrap();
    let mut mcp: Value = serde_json::from_slice(MCP).unwrap();
    let mut strategy = plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0].clone();
    strategy["id"] = json!("alternative");
    plugin["extensions"]["ai.rkat.toolkit"]["strategies"]
        .as_array_mut()
        .unwrap()
        .push(strategy);
    let mut recipe = recipes["recipes"][0].clone();
    recipe["strategy"] = json!("alternative");
    recipe["plan"]["server"] = json!("alternative-server");
    recipes["recipes"].as_array_mut().unwrap().push(recipe);
    mcp["mcpServers"]["alternative-server"] =
        json!({"type": "streamable-http", "url": "https://example.org/chosen-mcp"});
    fs::write(&fixture.plugin, serde_json::to_vec(&plugin).unwrap()).unwrap();
    fs::write(&fixture.recipes, serde_json::to_vec(&recipes).unwrap()).unwrap();
    fs::write(&fixture.mcp, serde_json::to_vec(&mcp).unwrap()).unwrap();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    assert_ne!(
        saved["recipe"]["digest"],
        saved["recipe"]["package_content_digest"]
    );
    let before = success(&fixture.read("host-realm", "alice", digest));
    let selection = fixture.selection(1);
    let selection_path = fixture._temp.path().join("selection.json");
    fs::write(
        &selection_path,
        serde_json::to_vec_pretty(&selection).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("exported");
    let result = success(&fixture.export(digest, &selection_path, &destination));
    assert_eq!(result["status"], "exported");
    assert_eq!(result["recipe"], saved["recipe"]);
    assert_eq!(result["destination"], json!(destination));
    let typed: ActivationManifest = serde_json::from_value(selection).unwrap();
    assert_eq!(
        result["selection_digest"],
        json!(ContentDigest::of_bytes(&canonical_json(&typed).unwrap()))
    );
    let names = fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names, ["mcp.json".into(), "plugin.json".into()].into());
    for name in ["plugin.json", "mcp.json"] {
        let bytes = fs::read(destination.join(name)).unwrap();
        assert_eq!(
            result["files"][name],
            json!({"sha256": ContentDigest::of_bytes(&bytes), "bytes": bytes.len()})
        );
    }
    let exported: Value =
        serde_json::from_slice(&fs::read(destination.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(
        exported["mcpServers"],
        json!({"alternative-server": mcp["mcpServers"]["alternative-server"]})
    );
    let exported_plugin: Value =
        serde_json::from_slice(&fs::read(destination.join("plugin.json")).unwrap()).unwrap();
    assert_eq!(exported_plugin["extensions"], plugin["extensions"]);
    assert_eq!(
        success(&fixture.read("host-realm", "alice", digest)),
        before
    );
}

#[test]
fn export_refusals_leave_no_destination_and_do_not_echo_selection_values() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let before = success(&fixture.read("host-realm", "alice", digest));
    let selection_path = fixture._temp.path().join("selection.json");
    let destination = fixture._temp.path().join("must-not-exist");
    for mutation in [
        "omitted",
        "empty",
        "extra",
        "package",
        "endpoint",
        "required-contract",
        "auth",
        "invalid-json",
    ] {
        let mut selection = fixture.selection(0);
        match mutation {
            "omitted" => {
                selection
                    .as_object_mut()
                    .unwrap()
                    .remove("portable_mcp_entries");
            }
            "empty" => selection["portable_mcp_entries"] = json!([]),
            "extra" => selection["portable_mcp_entries"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name": "PRIVATE_VALUE_MUST_NOT_APPEAR", "component": "remote"})),
            "package" => selection["package_content_digest"] = json!("0".repeat(64)),
            "endpoint" => {
                selection["components"][0]["source"]["endpoint"] =
                    json!("https://example.org/PRIVATE_VALUE_MUST_NOT_APPEAR")
            }
            "required-contract" => {
                selection["required_contracts"] = json!([{"name": "toolkit/required", "version": {"major": 1, "minimum_minor": 0}}])
            }
            "auth" => selection["exports"][0]["authentication"] = json!({"kind": "none"}),
            "invalid-json" => {}
            _ => unreachable!(),
        }
        let bytes = if mutation == "invalid-json" {
            b"{PRIVATE_VALUE_MUST_NOT_APPEAR".to_vec()
        } else {
            serde_json::to_vec(&selection).unwrap()
        };
        fs::write(&selection_path, bytes).unwrap();
        let output = failure(
            &fixture.export(digest, &selection_path, &destination),
            if mutation == "invalid-json" {
                "invalid_selection"
            } else {
                "portable_export_refused"
            },
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_VALUE_MUST_NOT_APPEAR"));
        assert!(!destination.exists(), "created output for {mutation}");
    }
    assert_eq!(
        success(&fixture.read("host-realm", "alice", digest)),
        before
    );
}

#[test]
fn export_requires_the_exact_operator_scope_recipe_identity_and_bundle_digest() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let selection_path = fixture._temp.path().join("selection.json");
    fs::write(
        &selection_path,
        serde_json::to_vec(&fixture.selection(0)).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("must-not-exist");
    for (flag, value, expected) in [
        ("--realm", "another-realm", "recipe_not_found"),
        ("--owner", "bob", "recipe_not_found"),
        ("--name", "another-package", "recipe_not_found"),
        (
            "--digest",
            saved["recipe"]["package_content_digest"].as_str().unwrap(),
            "stored_recipe_unverified",
        ),
    ] {
        let mut args = fixture.export(digest, &selection_path, &destination);
        let index = args.iter().position(|arg| arg == flag).unwrap() + 1;
        args[index] = value.into();
        failure(&args, expected);
        assert!(!destination.exists());
    }
    let mut args = fixture.export(digest, &selection_path, &destination);
    let database_index = args.iter().position(|arg| arg == "--database").unwrap() + 1;
    let absent_database = fixture._temp.path().join("absent.sqlite");
    args[database_index] = path(&absent_database);
    failure(&args, "database_not_found");
    assert!(!absent_database.exists());
    assert!(!destination.exists());
}

#[test]
fn export_never_overwrites_an_existing_destination() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let selection_path = fixture._temp.path().join("selection.json");
    fs::write(
        &selection_path,
        serde_json::to_vec(&fixture.selection(0)).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("already-owned");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("plugin.json"), b"existing owner bytes").unwrap();
    failure(
        &fixture.export(
            saved["recipe"]["digest"].as_str().unwrap(),
            &selection_path,
            &destination,
        ),
        "export_write_unconfirmed",
    );
    assert_eq!(
        fs::read(destination.join("plugin.json")).unwrap(),
        b"existing owner bytes"
    );
    assert!(!destination.join("mcp.json").exists());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn export_does_not_promote_a_retained_guidance_recipe() {
    let fixture = Fixture::new();
    let mut selected = fixture.selection(0);
    let mut plugin: Value = serde_json::from_slice(&fs::read(&fixture.plugin).unwrap()).unwrap();
    let mut recipes: Value = serde_json::from_slice(RECIPES).unwrap();
    plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0]["kind"] =
        json!("configuration_recipe");
    recipes["recipes"][0]["plan"] = json!({"kind": "guidance_only", "blockers": ["Explicit host deployment remains unresolved"]});
    fs::write(&fixture.plugin, serde_json::to_vec(&plugin).unwrap()).unwrap();
    fs::write(&fixture.recipes, serde_json::to_vec(&recipes).unwrap()).unwrap();
    let mut mcp: Value = serde_json::from_slice(MCP).unwrap();
    mcp["mcpServers"] = json!({});
    fs::write(&fixture.mcp, serde_json::to_vec(&mcp).unwrap()).unwrap();
    selected["package_content_digest"] =
        json!(fixture.bundle().package().content_digest().unwrap());
    let saved = success(&fixture.save());
    let selection_path = fixture._temp.path().join("selection.json");
    fs::write(&selection_path, serde_json::to_vec(&selected).unwrap()).unwrap();
    let destination = fixture._temp.path().join("must-not-exist");
    failure(
        &fixture.export(
            saved["recipe"]["digest"].as_str().unwrap(),
            &selection_path,
            &destination,
        ),
        "portable_export_refused",
    );
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn check_export_observes_missing_matching_and_changed_files_in_fresh_processes() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let selection = fixture._temp.path().join("activation.json");
    fs::write(
        &selection,
        serde_json::to_vec(&fixture.selection(0)).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("export");
    let arguments = fixture.check_export(digest, &selection, &destination);
    let absent = success(&arguments);
    assert_eq!(absent["status"], "inspected");
    assert_eq!(absent["recipe"], saved["recipe"]);
    assert_eq!(absent["export_check"]["state"], "missing");
    assert!(!destination.exists());

    let exported = success(&fixture.export(digest, &selection, &destination));
    let original = fs::read(destination.join("mcp.json")).unwrap();
    let original_time = fs::metadata(destination.join("mcp.json"))
        .unwrap()
        .modified()
        .unwrap();
    for _ in 0..2 {
        let checked = success(&arguments);
        assert_eq!(checked["status"], "inspected");
        assert_eq!(checked["selection_digest"], exported["selection_digest"]);
        assert_eq!(checked["destination"], exported["destination"]);
        assert_eq!(checked["export_check"]["state"], "matches");
        assert_eq!(
            checked["export_check"]["inventory"],
            json!({
                "state": "matches", "complete": true, "unexpected_entries": 0,
            })
        );
        for name in ["plugin.json", "mcp.json"] {
            let mut fingerprint = exported["files"][name].clone();
            // Export summaries identify content. Current Unix inspection also
            // observes executable intent; it does not change that summary.
            fingerprint["executable"] = json!(false);
            assert_eq!(
                checked["export_check"]["files"][name],
                json!({
                    "state": "matches", "expected": fingerprint,
                    "observed": fingerprint,
                })
            );
        }
    }
    assert_eq!(fs::read(destination.join("mcp.json")).unwrap(), original);
    assert_eq!(
        fs::metadata(destination.join("mcp.json"))
            .unwrap()
            .modified()
            .unwrap(),
        original_time
    );
    fs::write(destination.join("mcp.json"), b"changed").unwrap();
    let changed = success(&arguments);
    assert_eq!(changed["export_check"]["state"], "mismatch");
    let mut expected = exported["files"]["mcp.json"].clone();
    expected["executable"] = json!(false);
    assert_eq!(
        changed["export_check"]["files"]["mcp.json"]["expected"],
        expected
    );
    assert_eq!(
        changed["export_check"]["files"]["mcp.json"]["observed"],
        json!({
            "sha256": ContentDigest::of_bytes(b"changed"), "bytes": 7, "executable": false,
        })
    );
    fs::remove_file(destination.join("mcp.json")).unwrap();
    assert_eq!(success(&arguments)["export_check"]["state"], "missing");
    fs::write(destination.join("unexpected-private-name"), b"not returned").unwrap();
    let extra = success(&arguments);
    assert_eq!(extra["export_check"]["state"], "mismatch");
    assert_eq!(extra["export_check"]["inventory"]["unexpected_entries"], 1);
    assert!(!extra.to_string().contains("unexpected-private-name"));
    assert!(!extra.to_string().contains("not returned"));
    assert_eq!(
        success(&fixture.read("host-realm", "alice", digest))["recipe"]["origin"],
        saved["recipe"]["origin"]
    );
}

#[test]
fn check_export_does_not_create_a_missing_store_or_relax_exact_selection() {
    let fixture = Fixture::new();
    let selection = fixture._temp.path().join("activation.json");
    fs::write(
        &selection,
        serde_json::to_vec(&fixture.selection(0)).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("export");
    let digest = fixture.bundle().digest().to_string();
    failure(
        &fixture.check_export(&digest, &selection, &destination),
        "database_not_found",
    );
    assert!(!fixture.database.exists());
    assert!(!destination.exists());
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let mut wrong_scope = fixture.check_export(digest, &selection, &destination);
    let owner = wrong_scope.iter().position(|arg| arg == "--owner").unwrap() + 1;
    wrong_scope[owner] = "someone-else".into();
    failure(&wrong_scope, "recipe_not_found");
    let mut selected = fixture.selection(0);
    selected["components"][0]["source"]["endpoint"] = json!("https://wrong.invalid/mcp");
    fs::write(&selection, serde_json::to_vec(&selected).unwrap()).unwrap();
    failure(
        &fixture.check_export(digest, &selection, &destination),
        "portable_export_refused",
    );
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn check_export_rejects_root_links_and_fifo_without_reading_or_repairing() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let selection = fixture._temp.path().join("activation.json");
    fs::write(
        &selection,
        serde_json::to_vec(&fixture.selection(0)).unwrap(),
    )
    .unwrap();
    let destination = fixture._temp.path().join("export");
    success(&fixture.export(digest, &selection, &destination));
    let link = fixture._temp.path().join("link");
    symlink(&destination, &link).unwrap();
    for suffix in ["", "/", "/.", "//"] {
        let mut alias = link.as_os_str().to_owned();
        alias.push(suffix);
        let unavailable = success(&fixture.check_export(digest, &selection, Path::new(&alias)));
        assert_eq!(unavailable["export_check"]["state"], "unavailable");
        assert_eq!(
            unavailable["export_check"]["files"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
    }
    fs::remove_file(destination.join("mcp.json")).unwrap();
    assert!(
        Command::new("/usr/bin/mkfifo")
            .arg(destination.join("mcp.json"))
            .status()
            .unwrap()
            .success()
    );
    let fifo = success(&fixture.check_export(digest, &selection, &destination));
    assert_eq!(fifo["export_check"]["state"], "unavailable");
    assert!(
        !fifo["export_check"]["inventory"]["complete"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !fs::symlink_metadata(destination.join("mcp.json"))
            .unwrap()
            .is_file()
    );
}

#[test]
fn save_diagnostics_identify_document_errors_without_opening_store_or_echoing_values() {
    let canary = "PRIVATE_DIAGNOSTIC_CANARY";
    for (document, bytes, code) in [
        ("mcp", b"{\"mcpServers\":{}}".to_vec(), "mcp_schema"),
        (
            "mcp",
            format!("{{\"$schema\":\"{canary}\",\"mcpServers\":{{}}}}").into_bytes(),
            "mcp_schema",
        ),
        (
            "mcp",
            b"{\"$schema\":null,\"mcpServers\":{}}".to_vec(),
            "mcp_schema",
        ),
        (
            "mcp",
            b"{\"$schema\":7,\"mcpServers\":{}}".to_vec(),
            "mcp_schema",
        ),
        (
            "mcp",
            format!("{{\"{canary}\":true}}").into_bytes(),
            "mcp_document",
        ),
        (
            "mcp",
            b"{\"$schema\":\"a\",\"$schema\":\"b\",\"mcpServers\":{}}".to_vec(),
            "mcp_document",
        ),
        (
            "plugin",
            format!("{{bad-json:{canary}}}").into_bytes(),
            "plugin_document",
        ),
        (
            "recipes",
            format!("{{bad-json:{canary}}}").into_bytes(),
            "recipe_document",
        ),
    ] {
        let fixture = Fixture::new();
        let path = match document {
            "mcp" => &fixture.mcp,
            "plugin" => &fixture.plugin,
            _ => &fixture.recipes,
        };
        fs::write(path, bytes).unwrap();
        let output = failure(&fixture.save(), "invalid_bundle");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["diagnostic"],
            json!({"phase":"before_store_open", "code":code})
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(canary));
        assert!(!fixture.database.exists());
        // Refusal remains pre-store even if an unrelated existing file occupies
        // the database path; bytes are untouched, not opened as SQLite.
        fs::write(&fixture.database, canary).unwrap();
        failure(&fixture.save(), "invalid_bundle");
        assert_eq!(fs::read_to_string(&fixture.database).unwrap(), canary);
    }
}

#[test]
fn list_page_preserves_legacy_summaries_and_cross_process_cursor_scope() {
    let fixture = Fixture::new();
    for index in 0..3 {
        let mut plugin: Value =
            serde_json::from_slice(&fs::read(&fixture.plugin).unwrap()).unwrap();
        plugin["extensions"]["ai.rkat.toolkit"]["identity"]["name"] =
            json!(format!("paged-{index}"));
        fs::write(&fixture.plugin, serde_json::to_vec(&plugin).unwrap()).unwrap();
        success(&fixture.save());
    }
    let all = success(&fixture.args("list", "host-realm", "alice"));
    let mut args = fixture.args("list-page", "host-realm", "alice");
    args.extend(["--limit".into(), "2".into()]);
    let first = success(&args);
    assert_eq!(first["status"], "listed_page");
    assert_eq!(first["request_cursor"], Value::Null);
    assert_eq!(first["limit"], 2);
    assert_eq!(first["recipes"].as_array().unwrap().len(), 2);
    assert_eq!(first, success(&args));
    let cursor = first["next_cursor"].as_str().unwrap();
    args.extend(["--cursor".into(), cursor.into()]);
    let second = success(&args);
    assert_eq!(second["request_cursor"], cursor);
    assert!(second["next_cursor"].is_null());
    let mut joined = first["recipes"].as_array().unwrap().clone();
    joined.extend(second["recipes"].as_array().unwrap().clone());
    assert_eq!(json!(joined), all["recipes"]);
    assert_eq!(all, success(&fixture.args("list", "host-realm", "alice")));
    let owner = args.iter().position(|arg| arg == "--owner").unwrap() + 1;
    args[owner] = "bob".into();
    failure(&args, "invalid_page");
}

#[test]
fn list_page_refuses_invalid_input_before_missing_database_and_never_creates_it() {
    let fixture = Fixture::new();
    let mut valid = fixture.args("list-page", "host-realm", "alice");
    valid.extend(["--limit".into(), "20".into()]);
    failure(&valid, "database_not_found");
    assert!(!fixture.database.exists());
    for limit in ["0", "51", "-1", "twenty"] {
        let mut args = fixture.args("list-page", "host-realm", "alice");
        args.extend(["--limit".into(), limit.into()]);
        failure(&args, "invalid_page");
        assert!(!fixture.database.exists());
    }
    for cursor in ["", "00", "not-a-cursor", "f"] {
        let mut args = valid.clone();
        args.extend(["--cursor".into(), cursor.into()]);
        failure(&args, "invalid_page");
        assert!(!fixture.database.exists());
    }
    let mut mixed = fixture.args("list", "host-realm", "alice");
    mixed.extend(["--limit".into(), "20".into()]);
    failure(&mixed, "invalid_arguments");
}

#[test]
fn omitted_decision_uses_durable_submission_provenance_across_fresh_processes() {
    let fixture = Fixture::new();
    let mut args = fixture.save();
    args.truncate(args.len() - 6);
    let saved = success(&args);
    assert_eq!(saved["status"], "retained");
    assert_eq!(saved["recipe"]["origin"]["kind"], "local_investigation");
    assert_eq!(saved["recipe"]["origin"]["decision"]["owner"], "toolkit.local-recipes.current-submission");
    assert_eq!(success(&args), saved);
    // A new set of source files gives the same durable identity and reference.
    let fresh = fixture._temp.path().join("fresh-inputs");
    fs::create_dir(&fresh).unwrap();
    for (flag, source) in [("--plugin", &fixture.plugin), ("--mcp", &fixture.mcp), ("--recipes", &fixture.recipes)] {
        let destination = fresh.join(source.file_name().unwrap());
        fs::copy(source, &destination).unwrap();
        let index = args.iter().position(|arg| arg == flag).unwrap() + 1;
        args[index] = path(&destination);
        fs::remove_file(source).unwrap();
    }
    assert_eq!(success(&args), saved);
    fs::remove_dir_all(&fresh).unwrap();
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let loaded = success(&fixture.read("host-realm", "alice", digest));
    assert_eq!(loaded["recipe"]["origin"], saved["recipe"]["origin"]);
    assert_eq!(loaded["recipe"]["documents"]["mcp_json"], std::str::from_utf8(MCP).unwrap());
    for (realm, owner) in [("other-realm", "alice"), ("host-realm", "bob")] {
        assert_eq!(success(&fixture.read(realm, owner, digest))["status"], "not_found");
    }
}

#[test]
fn a_partial_decision_is_not_silently_replaced_with_submission_provenance() {
    for supplied in [vec!["--decision-owner", "external"],
                     vec!["--decision-record", "record", "--decision-revision", "1"]] {
        let fixture = Fixture::new();
        let mut args = fixture.save();
        args.truncate(args.len() - 6);
        args.extend(supplied.into_iter().map(str::to_owned));
        failure(&args, "invalid_arguments");
        assert!(!fixture.database.exists());
    }
}

#[test]
fn inert_declaration_exports_and_reopens_without_activation_references() {
    let fixture = Fixture::new();
    let saved = success(&fixture.save());
    let digest = saved["recipe"]["digest"].as_str().unwrap();
    let mut declaration = fixture.selection(0);
    declaration.as_object_mut().unwrap().remove("configuration");
    declaration.as_object_mut().unwrap().remove("policy");
    declaration["components"][0]["source"].as_object_mut().unwrap().remove("configuration_revision");
    declaration["selection_kind"] = json!("remote_declaration");
    declaration["portable_mcp_entries"][0]["operation_tools"] = json!({"fixture-read":"provider.read"});
    let raw = serde_json::to_vec(&declaration).unwrap();
    let path = fixture._temp.path().join("declaration.json"); fs::write(&path, &raw).unwrap();
    let destination = fixture._temp.path().join("inert-export");
    let with_declaration = |mut args:Vec<String>| {
        let at = args.iter().position(|arg| arg == "--selection").unwrap();
        args[at] = "--declaration".into(); args
    };
    let exported = success(&with_declaration(fixture.export(digest, &path, &destination)));
    let checked = success(&with_declaration(fixture.check_export(digest, &path, &destination)));
    assert_eq!(exported["selection_kind"], "remote_declaration");
    assert_eq!(checked["selection_kind"], exported["selection_kind"]);
    assert_eq!(exported["declared_remote_service"], checked["declared_remote_service"]);
    assert_eq!(exported["declared_remote_service"]["declaration_digest"], exported["selection_digest"]);
    assert_eq!(exported["declared_remote_service"]["selection_kind"], "remote_declaration");
    assert!(exported["declared_remote_service"].get("manifest_digest").is_none());
    let typed:toolkit_package::RemoteExportDeclaration = serde_json::from_slice(&raw).unwrap();
    assert_eq!(exported["selection_digest"], json!(ContentDigest::of_bytes(&canonical_json(&typed).unwrap())));
    assert_eq!(fs::read(&path).unwrap(), raw);
    assert_eq!(success(&fixture.read("host-realm", "alice", digest))["recipe"]["digest"], saved["recipe"]["digest"]);
    let refused = fixture._temp.path().join("refused-form");
    failure(&fixture.export(digest, &path, &refused), "invalid_selection");
    let mut both = with_declaration(fixture.export(digest, &path, &refused));
    both.extend(["--selection".into(), path.to_str().unwrap().into()]);
    failure(&both, "invalid_arguments");
    assert!(!refused.exists());
    declaration["components"][0]["source"]["endpoint"] = json!("https://substituted.invalid/mcp");
    fs::write(&path, serde_json::to_vec(&declaration).unwrap()).unwrap();
    failure(&with_declaration(fixture.export(digest, &path, &refused)), "portable_export_refused");
    failure(&with_declaration(fixture.check_export(digest, &path, &destination)), "portable_export_refused");
    assert!(!refused.exists());
}
