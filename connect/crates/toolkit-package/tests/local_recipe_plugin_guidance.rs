#![cfg(feature = "local-recipes")]

use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use toolkit_package::*;
use toolkit_store::{RecordData, SqliteStore, StoreScope};

const PACKAGE: &[u8] = include_bytes!("fixtures/python-install/package.json");
const SELECTION: &[u8] = include_bytes!("fixtures/python-install/activation.json");
const GUIDANCE: &str = "# Station readings\nUse only confirmed rows.\nTemperatures are tenths of a degree. Räksmörgås.\n";

fn evidence(record: &str) -> EvidenceReference {
    EvidenceReference {
        owner: Id::new("historical-observations").unwrap(),
        record: Id::new(record).unwrap(),
        revision: Revision::new("1").unwrap(),
    }
}

fn scope(owner: &str) -> StoreScope {
    StoreScope {
        realm: "household".into(),
        owner: owner.into(),
    }
}

fn selection() -> RetainedLocalSelection {
    RetainedLocalSelection::parse(
        PACKAGE,
        SELECTION,
        LocalSelectionProfile::Python,
        Id::new("python-server").unwrap(),
        evidence("decision"),
        vec![evidence("mcp-check")],
    )
    .unwrap()
}

fn row(path: &str, text: &str, format: Option<&str>, summary: Value) -> Value {
    let source = ContentDigest::of_bytes(text.as_bytes());
    let inspection = format.map(|format| {
        json!({
            "schema_version": 1, "scope": "metadata_only", "format": format,
            "status": "accepted", "source_digest": source, "summary": summary, "diagnostics": [],
        })
    });
    json!({"path":path, "text":text, "byte_length":text.len(), "source_digest":source,
           "inspection":inspection})
}

fn source(guidance: &str) -> Value {
    let plugin =
        serde_json::to_string(&json!({"$schema":AGENT_PLUGIN_SCHEMA,"name":"station-source"}))
            .unwrap();
    let mcp = serde_json::to_string(&json!({"$schema":AGENT_PLUGIN_MCP_SCHEMA,
        "mcpServers":{"station":{"type":"stdio","command":"never-execute-this-program","args":[]}}})).unwrap();
    let rows = vec![
        row(
            "./plugin.json",
            &plugin,
            Some("agent-plugin"),
            json!({
            "name":"station-source","version":null,"extension_namespaces":[],"toolkit":null}),
        ),
        row(
            "./mcp.json",
            &mcp,
            Some("mcp-servers"),
            json!({"accepted_server_count":1,
            "servers":[{"name":"station","transport":"stdio","argument_count":0,
                        "environment_fields":[],"working_directory_configured":false}]}),
        ),
        row("./skills/station/SKILL.md", guidance, None, Value::Null),
    ];
    json!({"schema_version":1,"scope":"observed_public_plugin_files","staged_id":"ordinary",
        "inspector_sha256":ContentDigest::of_bytes(b"historical-inspector-identity"),
        "report":{"schema_version":1,"scope":"metadata_only","format":"agent-plugin-files",
                  "status":"accepted","source_digest":ContentDigest::of_bytes(&canonical_json(&rows).unwrap()),
                  "files":rows}})
}

fn encode(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn rehash(value: &mut Value) {
    for row in value["report"]["files"].as_array_mut().unwrap() {
        let text = row["text"].as_str().unwrap().to_owned();
        row["byte_length"] = json!(text.len());
        row["source_digest"] = json!(ContentDigest::of_bytes(text.as_bytes()));
        if !row["inspection"].is_null() {
            row["inspection"]["source_digest"] = row["source_digest"].clone();
        }
    }
    value["report"]["source_digest"] = json!(ContentDigest::of_bytes(
        &canonical_json(&value["report"]["files"]).unwrap()
    ));
}

#[tokio::test]
async fn guidance_survives_source_removal_and_store_reopen_with_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("recipes.sqlite");
    let source_path = temp.path().join("plugin-source.json");
    let bytes = encode(&source(GUIDANCE));
    fs::write(&source_path, &bytes).unwrap();
    let selected = selection()
        .with_plugin_source(&fs::read(&source_path).unwrap())
        .unwrap();
    {
        let store = SqliteStore::open(&database).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let saved = recipes
            .save_retained_selection(selected.clone())
            .await
            .unwrap();
        assert_eq!(saved.plugin_source_json(), Some(bytes.as_slice()));
        assert_eq!(
            recipes
                .save_retained_selection(selected.clone())
                .await
                .unwrap()
                .digest(),
            saved.digest()
        );
        let summary = RetainedLocalRecipe::Selection(saved).summary().unwrap();
        assert_eq!(
            summary["plugin_source_digest"],
            json!(ContentDigest::of_bytes(&bytes))
        );
    }
    fs::remove_file(source_path).unwrap();
    let store = SqliteStore::open_existing(&database).unwrap();
    let loaded = LocalRecipeStore::new(&store, scope("alice"))
        .unwrap()
        .load_selection_exact(&selected.package().identity, selected.digest())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.plugin_source_json(), Some(bytes.as_slice()));
    assert_eq!(loaded.package_json(), PACKAGE);
    assert_eq!(loaded.selection_json(), SELECTION);
    assert!(
        LocalRecipeStore::new(&store, scope("bob"))
            .unwrap()
            .load_selection_exact(&selected.package().identity, selected.digest())
            .await
            .unwrap()
            .is_none()
    );
    for changed in [
        selection(),
        selection()
            .with_plugin_source(&encode(&source("Changed guidance.\n")))
            .unwrap(),
    ] {
        assert!(matches!(
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_retained_selection(changed)
                .await,
            Err(LocalRecipeError::ImmutableConflict)
        ));
    }
}

#[tokio::test]
async fn source_omission_preserves_legacy_digest_and_cannot_enrich_an_immutable_record() {
    let old = selection();
    let body = json!({"identity":old.package().identity,
        "package_content_digest":old.package().content_digest().unwrap(),
        "manifest_digest":old.validated_selection().digest(),
        "package_json":std::str::from_utf8(PACKAGE).unwrap(),
        "selection_json":std::str::from_utf8(SELECTION).unwrap(),
        "profile":"python","component":"python-server","decision":evidence("decision"),
        "verification":[evidence("mcp-check")]});
    assert_eq!(
        old.digest(),
        &ContentDigest::of_bytes(&canonical_json(&(LOCAL_RECIPE_SCHEMA, 2u32, body)).unwrap())
    );
    assert!(old.plugin_source_json().is_none());
    assert!(
        RetainedLocalRecipe::Selection(old.clone())
            .summary()
            .unwrap()
            .get("plugin_source_digest")
            .is_none()
    );
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    recipes.save_retained_selection(old.clone()).await.unwrap();
    assert!(matches!(
        recipes
            .save_retained_selection(old.with_plugin_source(&encode(&source(GUIDANCE))).unwrap())
            .await,
        Err(LocalRecipeError::ImmutableConflict)
    ));
}

#[test]
fn invalid_source_never_becomes_a_valid_selection_even_with_repaired_outer_hashes() {
    for mutation in [
        "wrapper-version",
        "wrapper-scope",
        "staged-path",
        "inspector-digest",
        "report-version",
        "report-format",
        "report-status",
        "missing-plugin",
        "duplicate-file",
        "parent-path",
        "colon-path",
        "backslash-path",
        "file-size",
        "total-size",
        "control",
        "plugin-json",
        "mcp-schema",
        "toolkit-contract",
        "inspection-format",
        "inspection-status",
        "inspection-summary",
        "unexpected-inspection",
        "unknown-field",
    ] {
        let mut value = source(GUIDANCE);
        match mutation {
            "wrapper-version" => value["schema_version"] = json!(2),
            "wrapper-scope" => value["scope"] = json!("trusted"),
            "staged-path" => value["staged_id"] = json!("../ordinary"),
            "inspector-digest" => value["inspector_sha256"] = json!("invalid"),
            "report-version" => value["report"]["schema_version"] = json!(2),
            "report-format" => value["report"]["format"] = json!("other"),
            "report-status" => value["report"]["status"] = json!("accepted_with_diagnostics"),
            "missing-plugin" => { value["report"]["files"].as_array_mut().unwrap().remove(0); },
            "duplicate-file" => { let row=value["report"]["files"][0].clone(); value["report"]["files"].as_array_mut().unwrap().push(row); },
            "parent-path" => value["report"]["files"][2]["path"] = json!("./../SKILL.md"),
            "colon-path" => value["report"]["files"][2]["path"] = json!("./C:/SKILL.md"),
            "backslash-path" => value["report"]["files"][2]["path"] = json!("./skills\\SKILL.md"),
            "file-size" => value["report"]["files"][2]["text"] = json!("x".repeat(32769)),
            "total-size" => { for n in 0..5 { value["report"]["files"].as_array_mut().unwrap().push(row(&format!("./extra{n}.md"), &"x".repeat(32768), None, Value::Null)); } },
            "control" => value["report"]["files"][2]["text"] = json!("control\u{7f}"),
            "plugin-json" => value["report"]["files"][0]["text"] = json!("not JSON"),
            "mcp-schema" => value["report"]["files"][1]["text"] = json!(r#"{"$schema":"unknown","mcpServers":{}}"#),
            "toolkit-contract" => value["report"]["files"][0]["text"] = json!(serde_json::to_string(&json!({"$schema":AGENT_PLUGIN_SCHEMA,"name":"station-source","extensions":{"ai.rkat.toolkit":{"unsupported":true}}})).unwrap()),
            "inspection-format" => value["report"]["files"][0]["inspection"]["format"] = json!("mcp-servers"),
            "inspection-status" => value["report"]["files"][0]["inspection"]["status"] = json!("accepted_with_diagnostics"),
            "inspection-summary" => value["report"]["files"][0]["inspection"]["summary"] = json!(null),
            "unexpected-inspection" => value["report"]["files"][2]["inspection"] = value["report"]["files"][0]["inspection"].clone(),
            "unknown-field" => value["unexpected"] = json!(true),
            _ => unreachable!(),
        }
        rehash(&mut value);
        assert!(
            selection().with_plugin_source(&encode(&value)).is_err(),
            "{mutation}"
        );
    }
    for mutation in [
        "file-hash",
        "file-length",
        "aggregate-hash",
        "inspection-hash",
    ] {
        let mut value = source(GUIDANCE);
        match mutation {
            "file-hash" => {
                value["report"]["files"][2]["source_digest"] =
                    json!(ContentDigest::of_bytes(b"other"))
            }
            "file-length" => value["report"]["files"][2]["byte_length"] = json!(1),
            "aggregate-hash" => {
                value["report"]["source_digest"] = json!(ContentDigest::of_bytes(b"other"))
            }
            "inspection-hash" => {
                value["report"]["files"][0]["inspection"]["source_digest"] =
                    json!(ContentDigest::of_bytes(b"other"))
            }
            _ => unreachable!(),
        }
        // File-level validation must also fail after the aggregate digest is repaired.
        if mutation != "aggregate-hash" {
            value["report"]["source_digest"] = json!(ContentDigest::of_bytes(
                &canonical_json(&value["report"]["files"]).unwrap()
            ));
        }
        assert!(
            selection().with_plugin_source(&encode(&value)).is_err(),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn changed_stored_guidance_is_refused_even_when_store_digest_is_recomputed() {
    for self_consistent in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        let selected = selection()
            .with_plugin_source(&encode(&source(GUIDANCE)))
            .unwrap();
        {
            let store = SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_retained_selection(selected.clone())
                .await
                .unwrap();
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        let raw: String = db
            .query_row("SELECT data FROM toolkit_records", [], |row| row.get(0))
            .unwrap();
        let mut data: RecordData = serde_json::from_str(&raw).unwrap();
        let mut changed: Value =
            serde_json::from_str(data.body["plugin_source_json"].as_str().unwrap()).unwrap();
        changed["report"]["files"][2]["text"] = json!("Replaced after save.\n");
        if self_consistent {
            rehash(&mut changed);
        }
        data.body["plugin_source_json"] = json!(String::from_utf8(encode(&changed)).unwrap());
        db.execute(
            "UPDATE toolkit_records SET data=?1,digest=?2",
            rusqlite::params![
                serde_json::to_string(&data).unwrap(),
                data.digest().unwrap()
            ],
        )
        .unwrap();
        drop(db);
        let store = SqliteStore::open_existing(&path).unwrap();
        assert!(
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .load_selection_exact(&selected.package().identity, selected.digest())
                .await
                .is_err()
        );
    }
}

fn cli(args: &[String]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_toolkit-local-recipes"))
        .args(args)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn args(verb: &str, database: &Path) -> Vec<String> {
    vec![
        verb.into(),
        "--database".into(),
        database.to_str().unwrap().into(),
        "--realm".into(),
        "household".into(),
        "--owner".into(),
        "alice".into(),
    ]
}

#[test]
fn real_inspector_source_survives_cli_save_then_source_directory_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let staging = root.join("staging");
    let plugin = staging.join("ordinary");
    fs::create_dir_all(plugin.join("skills/station")).unwrap();
    fs::write(
        plugin.join("plugin.json"),
        serde_json::to_vec(&json!({"$schema":AGENT_PLUGIN_SCHEMA,"name":"station-source"}))
            .unwrap(),
    )
    .unwrap();
    fs::write(plugin.join("skills/station/SKILL.md"), GUIDANCE).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-package-inspect"))
        .arg("agent-plugin-files")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&serde_json::to_vec(&json!({"root":staging,"directory":"./ordinary","files":["./plugin.json","./skills/station/SKILL.md"]})).unwrap()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let source = encode(
        &json!({"schema_version":1,"scope":"observed_public_plugin_files","staged_id":"ordinary","inspector_sha256":ContentDigest::of_bytes(&fs::read(env!("CARGO_BIN_EXE_toolkit-package-inspect")).unwrap()),"report":report}),
    );
    for (name, bytes) in [
        ("package.json", PACKAGE),
        ("selection.json", SELECTION),
        ("source.json", source.as_slice()),
    ] {
        fs::write(root.join(name), bytes).unwrap();
    }
    fs::write(
        root.join("verification.json"),
        serde_json::to_vec(&vec![evidence("mcp-check")]).unwrap(),
    )
    .unwrap();
    let database = root.join("recipes.sqlite");
    let mut save = args("save", &database);
    for (key, value) in [
        ("--package", root.join("package.json")),
        ("--selection", root.join("selection.json")),
        ("--plugin-source", root.join("source.json")),
        ("--verification", root.join("verification.json")),
    ] {
        save.extend([key.into(), value.to_str().unwrap().into()]);
    }
    save.extend(
        [
            "--profile",
            "python",
            "--component",
            "python-server",
            "--decision-owner",
            "historical-observations",
            "--decision-record",
            "decision",
            "--decision-revision",
            "1",
        ]
        .map(str::to_owned),
    );
    let saved = cli(&save);
    assert_eq!(
        saved["recipe"]["plugin_source_digest"],
        json!(ContentDigest::of_bytes(&source))
    );
    assert_eq!(saved, cli(&save));
    // Invalid source is checked before a new SQLite database is opened.
    let refused_database = root.join("must-not-create.sqlite");
    let mut rejected = save.clone();
    rejected[2] = refused_database.to_str().unwrap().into();
    fs::write(root.join("source.json"), br#"{"schema_version":2}"#).unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_toolkit-local-recipes"))
        .args(&rejected)
        .env_clear()
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let refused: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(refused["error"], "invalid_plugin_source");
    assert!(!refused_database.exists());
    fs::remove_dir_all(staging).unwrap();
    for name in [
        "package.json",
        "selection.json",
        "source.json",
        "verification.json",
    ] {
        fs::remove_file(root.join(name)).unwrap();
    }
    let mut read = args("read", &database);
    read.extend(
        [
            "--publisher",
            "fixture",
            "--name",
            "python-server",
            "--version",
            "0.0.0-fixture",
            "--digest",
            saved["recipe"]["digest"].as_str().unwrap(),
        ]
        .map(str::to_owned),
    );
    let loaded = cli(&read);
    let mut wrong_scope = read.clone();
    wrong_scope[6] = "bob".into();
    assert_eq!(cli(&wrong_scope)["status"], "not_found");
    assert_eq!(
        loaded["recipe"]["documents"]["plugin_source_json"],
        json!(String::from_utf8(source).unwrap())
    );
    assert_eq!(
        loaded["recipe"]["documents"]["package_json"],
        json!(std::str::from_utf8(PACKAGE).unwrap())
    );
}

#[test]
fn duplicate_source_keys_and_oversized_raw_documents_are_rejected() {
    let good = String::from_utf8(encode(&source(GUIDANCE))).unwrap();
    let duplicate = good.replacen("{", "{\"schema_version\":1,", 1);
    assert!(
        selection()
            .with_plugin_source(duplicate.as_bytes())
            .is_err()
    );
    let mut oversized = good.into_bytes();
    oversized.resize(1024 * 1024 + 1, b' ');
    assert!(selection().with_plugin_source(&oversized).is_err());
}

#[tokio::test]
async fn node_and_mcpb_keep_configuration_and_optional_guidance_independent() {
    const NODE_PACKAGE: &[u8] = include_bytes!("fixtures/node-install/package.json");
    const NODE_SELECTION: &[u8] = include_bytes!("fixtures/node-install/activation.json");
    const CONFIG: &[u8] = b"{ \"toolsets\": \"navigation\" }\n";
    let mut manifest: Value = serde_json::from_slice(NODE_SELECTION).unwrap();
    manifest["configuration"]["content_digest"] = json!(ContentDigest::of_bytes(CONFIG));
    manifest["components"][0]["source"]["dependencies"] = json!({"kind":"self_contained"});
    manifest["components"][0]["source"]["entrypoint"] =
        json!({"executable":"server.mjs","arguments":["--toolsets=navigation"]});
    let mcpb = RetainedLocalSelection::parse_mcpb(
        NODE_PACKAGE,
        &encode(&manifest),
        CONFIG,
        Id::new("node-server").unwrap(),
        evidence("decision"),
        vec![evidence("mcp-check")],
    )
    .unwrap();
    let node = RetainedLocalSelection::parse(
        NODE_PACKAGE,
        NODE_SELECTION,
        LocalSelectionProfile::Node,
        Id::new("node-server").unwrap(),
        evidence("decision"),
        vec![evidence("mcp-check")],
    )
    .unwrap();
    for (index, selected) in [node, mcpb].into_iter().enumerate() {
        let original_config = selected.configuration_json().map(Vec::from);
        let source = encode(&source(GUIDANCE));
        let selected = selected.with_plugin_source(&source).unwrap();
        assert_eq!(selected.configuration_json(), original_config.as_deref());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        {
            let store = SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_retained_selection(selected.clone())
                .await
                .unwrap();
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        let data: String = db
            .query_row("SELECT data FROM toolkit_records", [], |row| row.get(0))
            .unwrap();
        let data: RecordData = serde_json::from_str(&data).unwrap();
        assert_eq!(data.version, if index == 0 { 2 } else { 3 });
        drop(db);
        let store = SqliteStore::open_existing(&path).unwrap();
        let loaded = LocalRecipeStore::new(&store, scope("alice"))
            .unwrap()
            .load_selection_exact(&selected.package().identity, selected.digest())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.configuration_json(), original_config.as_deref());
        assert_eq!(loaded.plugin_source_json(), Some(source.as_slice()));
    }
}

#[test]
fn maximum_file_text_fits_after_ascii_sidecar_encoding() {
    let mut value = source("");
    let rows = value["report"]["files"].as_array_mut().unwrap();
    let fixed: usize = rows
        .iter()
        .map(|row| row["text"].as_str().unwrap().len())
        .sum();
    let mut remaining = 128 * 1024 - fixed;
    while remaining > 0 {
        let length = remaining.min(32 * 1024);
        let mut text = "é".repeat(length / 2);
        if length % 2 == 1 {
            text.push('x');
        }
        rows.push(row(
            &format!("./guide{}.md", rows.len()),
            &text,
            None,
            Value::Null,
        ));
        remaining -= length;
    }
    rehash(&mut value);
    // The host emits ensure_ascii=True, which expands this two-byte UTF-8
    // character to six ASCII bytes. Preserve the identical parsed report.
    let ascii = String::from_utf8(encode(&value))
        .unwrap()
        .replace('é', "\\u00e9");
    assert!(ascii.len() > 256 * 1024);
    assert!(ascii.len() < 1024 * 1024);
    let selected = selection().with_plugin_source(ascii.as_bytes()).unwrap();
    assert_eq!(selected.plugin_source_json(), Some(ascii.as_bytes()));
}

#[tokio::test]
async fn an_explicit_null_source_is_not_silently_treated_as_legacy_omission() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let selected = selection();
    {
        let store = SqliteStore::open(&path).unwrap();
        LocalRecipeStore::new(&store, scope("alice"))
            .unwrap()
            .save_retained_selection(selected.clone())
            .await
            .unwrap();
    }
    let db = rusqlite::Connection::open(&path).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM toolkit_records", [], |row| row.get(0))
        .unwrap();
    let mut data: RecordData = serde_json::from_str(&raw).unwrap();
    data.body["plugin_source_json"] = Value::Null;
    db.execute(
        "UPDATE toolkit_records SET data=?1,digest=?2",
        rusqlite::params![
            serde_json::to_string(&data).unwrap(),
            data.digest().unwrap()
        ],
    )
    .unwrap();
    drop(db);
    let store = SqliteStore::open_existing(&path).unwrap();
    assert!(
        LocalRecipeStore::new(&store, scope("alice"))
            .unwrap()
            .load_selection_exact(&selected.package().identity, selected.digest())
            .await
            .is_err()
    );
}
