#![cfg(feature = "local-recipes")]

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use toolkit_package::{ContentDigest, IntegrationPackage, LocalSelectionProfile};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(
            root.join("package.json"),
            include_bytes!("fixtures/python-install/package.json"),
        )
        .unwrap();
        fs::write(
            root.join("selection.json"),
            include_bytes!("fixtures/python-install/activation.json"),
        )
        .unwrap();
        fs::write(
            root.join("verification.json"),
            serde_json::to_vec(&json!([
                {"owner": "fixture-observations", "record": "historical-install", "revision": "1"}
            ]))
            .unwrap(),
        )
        .unwrap();
        Self { _temp: temp, root }
    }

    fn args(&self, verb: &str) -> Vec<String> {
        vec![
            verb.into(),
            "--database".into(),
            self.root.join("recipes.sqlite").display().to_string(),
            "--realm".into(),
            "household".into(),
            "--owner".into(),
            "alice".into(),
        ]
    }

    fn save(&self) -> Vec<String> {
        let mut args = self.args("save");
        for (flag, file) in [
            ("--package", "package.json"),
            ("--selection", "selection.json"),
            ("--verification", "verification.json"),
        ] {
            args.extend([flag.into(), self.root.join(file).display().to_string()]);
        }
        args.extend(
            [
                "--profile",
                "python",
                "--component",
                "python-server",
                "--decision-owner",
                "fixture-decisions",
                "--decision-record",
                "selected-python",
                "--decision-revision",
                "1",
            ]
            .map(str::to_owned),
        );
        args
    }

    fn read(&self, digest: &str) -> Vec<String> {
        let mut args = self.args("read");
        args.extend(
            [
                "--publisher",
                "fixture",
                "--name",
                "python-server",
                "--version",
                "0.0.0-fixture",
                "--digest",
                digest,
            ]
            .map(str::to_owned),
        );
        args
    }
}

fn invoke(args: &[String], error: Option<&str>) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_toolkit-local-recipes"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert_eq!(
        result.status.success(),
        error.is_none(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(
        result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["scope"], "retained_recipe_metadata");
    if let Some(error) = error {
        assert_eq!(value["error"], error);
    }
    value
}

fn replace(args: &mut [String], flag: &str, value: &str) {
    let index = args.iter().position(|s| s == flag).unwrap();
    args[index + 1] = value.into();
}

#[test]
fn exact_local_choice_reopens_without_claiming_current_authority() {
    let fixture = Fixture::new();
    let saved = invoke(&fixture.save(), None);
    assert_eq!(saved["status"], "retained");
    let summary = &saved["recipe"];
    assert_eq!(summary["kind"], "local_selection");
    assert_eq!(summary["profile"], "python");
    assert_eq!(
        summary["reuse"],
        "requires_current_host_selection_and_artifact_validation"
    );
    assert_eq!(summary["verification"][0]["record"], "historical-install");
    assert_eq!(invoke(&fixture.save(), None), saved);
    let digest = summary["digest"].as_str().unwrap();
    let read = invoke(&fixture.read(digest), None);
    assert_eq!(read["status"], "found");
    for (key, file) in [
        ("package_json", "package.json"),
        ("selection_json", "selection.json"),
    ] {
        assert_eq!(
            read["recipe"]["documents"][key],
            fs::read_to_string(fixture.root.join(file)).unwrap()
        );
    }
    assert_eq!(
        invoke(&fixture.args("list"), None)["recipes"],
        json!([summary])
    );
    invoke(
        &fixture.read(&"0".repeat(64)),
        Some("stored_recipe_unverified"),
    );
    for (flag, scope) in [("--owner", "bob"), ("--realm", "other")] {
        let mut args = fixture.read(digest);
        replace(&mut args, flag, scope);
        assert_eq!(invoke(&args, None)["status"], "not_found");
    }
}

#[test]
fn changed_document_formatting_and_evidence_cannot_replace_an_identity() {
    let fixture = Fixture::new();
    let saved = invoke(&fixture.save(), None);
    let bytes = fs::read(fixture.root.join("selection.json")).unwrap();
    let mut changed = bytes.clone();
    changed.push(b'\n');
    fs::write(fixture.root.join("selection.json"), changed).unwrap();
    invoke(&fixture.save(), Some("immutable_conflict"));
    fs::write(fixture.root.join("selection.json"), bytes).unwrap();
    let mut args = fixture.save();
    replace(&mut args, "--decision-revision", "2");
    invoke(&args, Some("immutable_conflict"));
    fs::write(
        fixture.root.join("verification.json"),
        br#"[{"owner":"fixture-observations","record":"historical-install","revision":"2"}]"#,
    )
    .unwrap();
    invoke(&fixture.save(), Some("immutable_conflict"));
    let read = invoke(
        &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
        None,
    );
    assert_eq!(read["recipe"]["verification"][0]["revision"], "1");
}

#[test]
fn invalid_or_mixed_inputs_are_refused_before_database_creation() {
    let fixture = Fixture::new();
    for profile in ["ruby", "Python", "remote"] {
        let mut args = fixture.save();
        replace(&mut args, "--profile", profile);
        invoke(&args, Some("unsupported_local_profile"));
    }
    let mut args = fixture.save();
    replace(&mut args, "--profile", "node");
    invoke(&args, Some("invalid_local_selection"));
    let mut args = fixture.save();
    replace(&mut args, "--component", "other");
    invoke(&args, Some("invalid_local_selection"));
    let mut args = fixture.save();
    args.extend([
        "--plugin".into(),
        fixture.root.join("package.json").display().to_string(),
    ]);
    invoke(&args, Some("invalid_arguments"));
    fs::write(fixture.root.join("verification.json"), b"[]").unwrap();
    invoke(&fixture.save(), Some("invalid_local_selection"));
    assert!(!fixture.root.join("recipes.sqlite").exists());
}

fn mcpb_fixture() -> Fixture {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("package.json"),
        include_bytes!("fixtures/node-install/package.json"),
    )
    .unwrap();
    let mut selection: Value =
        serde_json::from_slice(include_bytes!("fixtures/node-install/activation.json")).unwrap();
    let configuration = b"{\n  \"toolsets\": \"geometry\"\n}\n";
    selection["configuration"]["content_digest"] = json!(ContentDigest::of_bytes(configuration));
    selection["components"][0]["source"]["dependencies"] = json!({"kind":"self_contained"});
    fs::write(
        fixture.root.join("selection.json"),
        serde_json::to_vec(&selection).unwrap(),
    )
    .unwrap();
    fs::write(fixture.root.join("configuration.json"), configuration).unwrap();
    fixture
}

fn mcpb_save(fixture: &Fixture) -> Vec<String> {
    let mut args = fixture.save();
    replace(&mut args, "--profile", "mcpb");
    replace(&mut args, "--component", "node-server");
    args.extend([
        "--configuration".into(),
        fixture
            .root
            .join("configuration.json")
            .display()
            .to_string(),
    ]);
    args
}

#[test]
fn mcpb_roundtrip_preserves_exact_configuration_in_the_same_store() {
    let fixture = mcpb_fixture();
    let saved = invoke(&mcpb_save(&fixture), None);
    assert_eq!(saved["recipe"]["profile"], "mcpb");
    let mut args = fixture.read(saved["recipe"]["digest"].as_str().unwrap());
    replace(&mut args, "--name", "node-server");
    let found = invoke(&args, None);
    assert_eq!(
        found["recipe"]["documents"]["configuration_json"],
        fs::read_to_string(fixture.root.join("configuration.json")).unwrap()
    );
    assert_eq!(
        found["recipe"]["configuration_digest"],
        json!(ContentDigest::of_bytes(
            &fs::read(fixture.root.join("configuration.json")).unwrap()
        ))
    );
    assert_eq!(invoke(&mcpb_save(&fixture), None), saved);
    assert_eq!(
        invoke(&fixture.args("list"), None)["recipes"],
        json!([saved["recipe"].clone()])
    );
}

#[test]
fn mcpb_configuration_is_required_bound_and_exclusive_before_opening_store() {
    let fixture = mcpb_fixture();
    let mut args = mcpb_save(&fixture);
    args.truncate(args.len() - 2);
    invoke(&args, Some("invalid_arguments"));
    // The Python selection would already fail, so use a valid Python fixture
    // to prove that this extra flag cannot be silently ignored.
    let python = Fixture::new();
    let mut python_args = python.save();
    python_args.extend([
        "--configuration".into(),
        fixture
            .root
            .join("configuration.json")
            .display()
            .to_string(),
    ]);
    invoke(&python_args, Some("invalid_arguments"));
    assert!(!python.root.join("recipes.sqlite").exists());
    fs::write(
        fixture.root.join("configuration.json"),
        b"{\"toolsets\":\"changed\"}",
    )
    .unwrap();
    invoke(&mcpb_save(&fixture), Some("invalid_local_selection"));
    // Bind duplicate-key bytes to isolate syntax refusal from digest refusal.
    let duplicate = b"{\"toolsets\":\"one\",\"toolsets\":\"two\"}";
    fs::write(fixture.root.join("configuration.json"), duplicate).unwrap();
    let mut selection: Value =
        serde_json::from_slice(&fs::read(fixture.root.join("selection.json")).unwrap()).unwrap();
    selection["configuration"]["content_digest"] = json!(ContentDigest::of_bytes(duplicate));
    fs::write(
        fixture.root.join("selection.json"),
        serde_json::to_vec(&selection).unwrap(),
    )
    .unwrap();
    invoke(&mcpb_save(&fixture), Some("invalid_local_selection"));
    assert!(!fixture.root.join("recipes.sqlite").exists());
}

#[test]
fn local_metadata_does_not_become_a_remote_export() {
    let fixture = Fixture::new();
    let saved = invoke(&fixture.save(), None);
    let mut args = fixture.read(saved["recipe"]["digest"].as_str().unwrap());
    args[0] = "export".into();
    args.extend([
        "--selection".into(),
        fixture.root.join("selection.json").display().to_string(),
        "--destination".into(),
        fixture.root.join("must-not-exist").display().to_string(),
    ]);
    invoke(&args, Some("stored_recipe_unverified"));
    assert!(!fixture.root.join("must-not-exist").exists());
}

const PYTHON_PACKAGE: &[u8] = include_bytes!("fixtures/python-install/package.json");
const NODE_PACKAGE: &[u8] = include_bytes!("fixtures/node-install/package.json");

// Explicit software declaration fixture, independent of ActivationManifest.
fn software_documents(profile: LocalSelectionProfile) -> (&'static [u8], Vec<u8>, &'static str) {
    let (bytes, runtime, component, strategy, location, executable, artifacts) = match profile {
        LocalSelectionProfile::Python => (PYTHON_PACKAGE, "python", "python-server", "python-local",
            "https://example.invalid/fixture_server-1.0-py3-none-any.whl", "fixture_server/__main__.py",
            json!({"requirements":"2".repeat(64),"dependency_one-1.0-py3-none-any.whl":"3".repeat(64)})),
        LocalSelectionProfile::Node => (NODE_PACKAGE, "node", "node-server", "node-local",
            "https://example.invalid/node-server.tgz", "dist/index.js",
            json!({"package-json":"2".repeat(64),"package-lock":"3".repeat(64),"dependency-archive":"4".repeat(64)})),
        LocalSelectionProfile::Mcpb => panic!("full MCPB path only"),
    };
    let package: IntegrationPackage = serde_json::from_slice(bytes).unwrap();
    let value = json!({"schema_version":1,"document_kind":"local_software_declaration",
        "package":package.identity,"package_content_digest":package.content_digest().unwrap(),"strategy":strategy,
        "components":[{"id":component,"version":"0.0.0-fixture","runtime":runtime,
            "source":{"kind":"artifact","location":location,"content_digest":"1".repeat(64),
                "dependencies":{"kind":"pinned_closure","artifacts":artifacts},
                "entrypoint":{"executable":executable,"arguments":[]}}}],
        "exports":[{"role":"service_connector","id":"service","components":[component],
            "authentication":{"kind":"none"},"operations":[{"id":"fixture-read","schema_digest":"5".repeat(64),
                "declared_effect":"read","required_authority":["read"],"resources":{"kind":"connection_wide"},
                "result_limits":{"maximum_bytes":4096,"maximum_items":10}}]}],
        "portable_mcp_entries":[{"name":"software-server","component":component,
            "operation_tools":{"fixture-read":"actual_read"}}]});
    (bytes, serde_json::to_vec_pretty(&value).unwrap(), component)
}

#[test]
fn software_save_cli_retains_exact_new_form_for_python_and_node_and_refuses_cross_form_flags() {
    for profile in [LocalSelectionProfile::Python,LocalSelectionProfile::Node] {
        let fixture=Fixture::new();
        let (package,declaration,component)=software_documents(profile);
        fs::write(fixture.root.join("package.json"),package).unwrap();
        fs::write(fixture.root.join("declaration.json"),&declaration).unwrap();
        let mut args=fixture.save();
        let selection=args.iter().position(|a| a=="--selection").unwrap();
        args[selection]="--declaration".into();
        args[selection+1]=fixture.root.join("declaration.json").display().to_string();
        replace(&mut args,"--profile",if profile==LocalSelectionProfile::Python {"python"} else {"node"});
        replace(&mut args,"--component",component);
        let mut both=args.clone(); both.extend(["--selection".into(),fixture.root.join("selection.json").display().to_string()]);
        invoke(&both,Some("invalid_arguments"));
        let mut wrong=args.clone();wrong[selection]="--selection".into();
        invoke(&wrong,Some("invalid_local_selection"));
        assert!(!fixture.root.join("recipes.sqlite").exists());
        let saved=invoke(&args,None);
        assert_eq!(saved["recipe"]["kind"],"local_software_selection");
        assert_eq!(saved["recipe"]["document_kind"],"local_software_declaration");
        assert!(saved["recipe"].get("manifest_digest").is_none());
        assert!(saved["recipe"].get("configuration_digest").is_none());
        assert_eq!(invoke(&args,None),saved);
        let mut read=fixture.read(saved["recipe"]["digest"].as_str().unwrap());
        replace(&mut read,"--name",component);
        let reopened=invoke(&read,None);
        assert_eq!(reopened["recipe"]["documents"]["declaration_json"],String::from_utf8(declaration).unwrap());
        assert_eq!(reopened["recipe"]["documents"]["package_json"],std::str::from_utf8(package).unwrap());
        assert!(reopened["recipe"]["documents"].get("selection_json").is_none());
        assert_eq!(invoke(&fixture.args("list"),None)["recipes"],json!([saved["recipe"].clone()]));
    }
}

// This helper calls the actual canonical binary. Source rows are not a mocked
// inspector envelope, and no command from the inspected MCP document is run.
fn retained_guidance_source(fixture: &Fixture) -> (Value, Vec<String>, Value) {
    use std::io::Write;
    use std::process::Stdio;
    let source = fixture.root.join("ordinary");
    let skill = "skills/observed-guide";
    fs::create_dir_all(source.join(format!("{skill}/data"))).unwrap();
    let mut toolkit_package: Value = serde_json::from_slice(PYTHON_PACKAGE).unwrap();
    toolkit_package["strategies"][0]["known_requirements"] = json!([{
        "name": "fixture/required-service", "version": {"major": 1, "minimum_minor": 0},
        "authority": ["read"], "availability": {"kind": "required"}
    }]);
    let plugin = json!({
        "$schema": toolkit_package::AGENT_PLUGIN_SCHEMA, "name": "retained.guidance",
        "extensions": {
            "ai.rkat.toolkit": toolkit_package,
            "org.example.other-client": {"notice": "opaque namespace retained"}
        }
    });
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec_pretty(&plugin).unwrap(),
    )
    .unwrap();
    fs::write(
        source.join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": toolkit_package::AGENT_PLUGIN_MCP_SCHEMA,
            "mcpServers": {"never-exported": {"type": "stdio", "command": "./bin/not-retained"}}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(source.join(format!("{skill}/SKILL.md")),
        b"---\nname: observed-guide\ndescription: Read retained public guidance.\n---\nRead [the data](data/example.csv).\n").unwrap();
    let reading = format!(
        "label,value\nretained,{}\n",
        fixture.root.file_name().unwrap().to_str().unwrap()
    );
    fs::write(source.join(format!("{skill}/data/example.csv")), reading).unwrap();
    fs::write(
        source.join(format!("{skill}/data/example.py")),
        b"# Public example, never an exported entrypoint.\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            source.join(format!("{skill}/data/example.py")),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    fs::write(source.join("root.txt"), b"unselected root asset\n").unwrap();
    let selected = vec![
        format!("./{skill}/SKILL.md"),
        format!("./{skill}/data/example.csv"),
        format!("./{skill}/data/example.py"),
    ];
    let files = [
        vec![
            "./plugin.json".to_owned(),
            "./mcp.json".to_owned(),
            "./root.txt".to_owned(),
        ],
        selected.clone(),
    ]
    .concat();
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-package-inspect"))
        .arg("agent-plugin-files")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(&json!({
                "root": fixture.root, "directory": "./ordinary", "files": files
            }))
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "accepted");
    let inspector = fs::read(env!("CARGO_BIN_EXE_toolkit-package-inspect")).unwrap();
    let observation = json!({"schema_version": 1, "scope": "observed_public_plugin_files",
        "staged_id": "ordinary", "inspector_sha256": ContentDigest::of_bytes(&inspector), "report": report});
    fs::write(
        fixture.root.join("plugin-source.json"),
        serde_json::to_vec(&observation).unwrap(),
    )
    .unwrap();
    (observation, selected, plugin)
}

fn save_guidance(fixture: &Fixture, software: bool) -> (Value, Value, Vec<String>, Value) {
    let (observation, files, plugin) = retained_guidance_source(fixture);
    let mut args = fixture.save();
    if software {
        let (_, declaration, _) = software_documents(LocalSelectionProfile::Python);
        fs::write(fixture.root.join("declaration.json"), declaration).unwrap();
        let index = args.iter().position(|arg| arg == "--selection").unwrap();
        args[index] = "--declaration".into();
        args[index + 1] = fixture.root.join("declaration.json").display().to_string();
    }
    args.extend([
        "--plugin-source".into(),
        fixture
            .root
            .join("plugin-source.json")
            .display()
            .to_string(),
    ]);
    let saved = invoke(&args, None);
    assert_eq!(invoke(&args, None), saved);
    (saved, observation, files, plugin)
}

fn guidance_args(fixture: &Fixture, saved: &Value, selection: &Value) -> Vec<String> {
    fs::write(
        fixture.root.join("guidance.json"),
        serde_json::to_vec(selection).unwrap(),
    )
    .unwrap();
    let mut args = fixture.read(saved["recipe"]["digest"].as_str().unwrap());
    args[0] = "export".into();
    args.extend([
        "--guidance".into(),
        fixture.root.join("guidance.json").display().to_string(),
        "--destination".into(),
        fixture.root.join("exported").display().to_string(),
    ]);
    args
}

#[test]
fn retained_guidance_exports_after_staging_removal_for_a_generic_reader() {
    for software in [false, true] {
        let fixture = Fixture::new();
        let (saved, observation, files, plugin) = save_guidance(&fixture, software);
        let before = invoke(
            &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
            None,
        );
        let selection = json!({"plugin_source_digest": saved["recipe"]["plugin_source_digest"], "files": files});
        let mut args = guidance_args(&fixture, &saved, &selection);
        fs::remove_dir_all(fixture.root.join("ordinary")).unwrap();
        fs::remove_file(fixture.root.join("plugin-source.json")).unwrap();
        let exported = invoke(&args, None);
        assert_eq!(exported["status"], "exported");
        assert_eq!(exported["selection_kind"], "guidance_only");
        assert_eq!(
            exported["plugin_source_digest"],
            selection["plugin_source_digest"]
        );
        assert_eq!(
            exported["selection_digest"],
            json!(ContentDigest::of_bytes(
                &toolkit_package::canonical_json(&selection).unwrap()
            ))
        );
        assert!(exported.get("declared_remote_service").is_none());
        let destination = fixture.root.join("exported");
        let manifest: Value =
            serde_json::from_slice(&fs::read(destination.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest, plugin);
        assert!(!destination.join("mcp.json").exists());
        assert!(!destination.join("root.txt").exists());
        assert!(!destination.join("bin").exists());
        // Independent generic reader: ordinary directory and text reads only,
        // ignoring extension namespaces and knowing no retained Toolkit schema.
        let skills = fs::read_dir(destination.join("skills"))
            .unwrap()
            .map(|row| row.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(skills.len(), 1);
        let guide = fs::read_to_string(skills[0].join("SKILL.md")).unwrap();
        let relative = guide.split("](").nth(1).unwrap().split(')').next().unwrap();
        let reference = skills[0].join(relative);
        let data = fs::read(&reference).unwrap();
        let retained_path = format!(
            "./{}",
            reference
                .strip_prefix(&destination)
                .unwrap()
                .to_str()
                .unwrap()
        );
        let retained = observation["report"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["path"] == retained_path)
            .unwrap();
        assert_eq!(data, retained["text"].as_str().unwrap().as_bytes());
        assert_eq!(
            exported["files"].as_object().unwrap().len(),
            files.len() + 1
        );
        for file in &files {
            let name = file.strip_prefix("./").unwrap();
            let bytes = fs::read(destination.join(name)).unwrap();
            let original = observation["report"]["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["path"] == *file)
                .unwrap();
            assert_eq!(bytes, original["text"].as_str().unwrap().as_bytes());
            assert_eq!(
                exported["files"][name],
                json!({"sha256": ContentDigest::of_bytes(&bytes), "bytes": bytes.len()})
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(destination.join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o111,
                    0
                );
            }
        }
        let manifest_bytes = fs::read(destination.join("plugin.json")).unwrap();
        assert_eq!(
            exported["files"]["plugin.json"],
            json!({"sha256": ContentDigest::of_bytes(&manifest_bytes), "bytes": manifest_bytes.len()})
        );
        args[0] = "check-export".into();
        let checked = invoke(&args, None);
        assert_eq!(checked["selection_kind"], "guidance_only");
        assert_eq!(
            checked["plugin_source_digest"],
            selection["plugin_source_digest"]
        );
        assert_eq!(checked["export_check"]["state"], "matches");
        assert_eq!(
            checked["export_check"]["inventory"],
            json!({"state": "matches", "complete": true, "unexpected_entries": 0})
        );
        assert_eq!(
            checked["export_check"]["files"].as_object().unwrap().len(),
            files.len() + 1
        );
        let changed = destination.join(files[1].strip_prefix("./").unwrap());
        fs::write(&changed, b"changed\n").unwrap();
        assert_eq!(invoke(&args, None)["export_check"]["state"], "mismatch");
        fs::write(changed, data).unwrap();
        fs::write(destination.join("unexpected.txt"), b"extra\n").unwrap();
        assert_eq!(
            invoke(&args, None)["export_check"]["inventory"]["unexpected_entries"],
            1
        );
        fs::remove_file(destination.join("unexpected.txt")).unwrap();
        assert_eq!(invoke(&args, None)["export_check"]["state"], "matches");
        assert_eq!(
            invoke(
                &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
                None
            ),
            before
        );
        args[0] = "export".into();
        invoke(&args, Some("export_write_unconfirmed"));
    }
}

#[test]
fn retained_guidance_refuses_foreign_changed_or_unselected_source_without_output() {
    let fixture = Fixture::new();
    let (saved, _, files, _) = save_guidance(&fixture, false);
    let before = invoke(
        &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
        None,
    );
    let selection =
        json!({"plugin_source_digest": saved["recipe"]["plugin_source_digest"], "files": files});
    let args = guidance_args(&fixture, &saved, &selection);
    for (flag, value, error) in [
        ("--owner", "bob", "recipe_not_found"),
        ("--realm", "elsewhere", "recipe_not_found"),
        ("--name", "not-retained", "recipe_not_found"),
        ("--digest", &"0".repeat(64), "stored_recipe_unverified"),
    ] {
        let mut bad = args.clone();
        replace(&mut bad, flag, value);
        invoke(&bad, Some(error));
    }
    let mut wrong_source = selection.clone();
    wrong_source["plugin_source_digest"] = json!("0".repeat(64));
    invoke(
        &guidance_args(&fixture, &saved, &wrong_source),
        Some("portable_export_refused"),
    );
    let invalid_files = vec![
        vec![],
        vec![files[1].clone()],
        vec![files[0].clone(), files[0].clone()],
        vec!["./plugin.json".into()],
        vec!["./mcp.json".into()],
        vec!["./root.txt".into()],
        vec![
            files[0].clone(),
            "./skills/observed-guide/missing.json".into(),
        ],
        vec![files[0].clone(), "./skills/other/data/example.csv".into()],
        vec!["./skills/observed-guide/../SKILL.md".into()],
        vec!["./skills/observed-guide/*.md".into()],
        vec!["./skills/observed-guide//SKILL.md".into()],
        vec!["./skills/observed-guide/bad\n".into()],
        vec!["./skills/observed-guide/a:b".into()],
        vec!["./skills/observed-guide/a\\b".into()],
        vec!["./skills/observed-guide/".to_owned() + &"a".repeat(1024)],
        vec!["./skills/observed-guide/".to_owned() + &"a/".repeat(14) + "last"],
        vec![files[0].clone(); 16],
    ];
    for files in invalid_files {
        let bad =
            json!({"plugin_source_digest": selection["plugin_source_digest"], "files": files});
        invoke(
            &guidance_args(&fixture, &saved, &bad),
            Some("portable_export_refused"),
        );
    }
    for flag in ["--selection", "--declaration"] {
        let mut both = guidance_args(&fixture, &saved, &selection);
        both.extend([
            flag.into(),
            fixture.root.join("selection.json").display().to_string(),
        ]);
        invoke(&both, Some("invalid_arguments"));
    }
    let mut contents = selection.clone();
    contents["text"] = json!("caller supplied contents");
    invoke(
        &guidance_args(&fixture, &saved, &contents),
        Some("invalid_selection"),
    );
    assert!(!fixture.root.join("exported").exists());
    assert_eq!(
        invoke(
            &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
            None
        ),
        before
    );
    let no_source = Fixture::new();
    let saved = invoke(&no_source.save(), None);
    invoke(
        &guidance_args(&no_source, &saved, &selection),
        Some("portable_export_refused"),
    );
    assert!(!no_source.root.join("exported").exists());
}

#[test]
fn retained_guidance_keeps_source_validation_and_refuses_selected_parent_collisions() {
    let fixture = Fixture::new();
    let (saved, observation, files, _) = save_guidance(&fixture, false);
    let before = invoke(
        &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
        None,
    );
    let mut save = fixture.save();
    save.extend([
        "--plugin-source".into(),
        fixture
            .root
            .join("plugin-source.json")
            .display()
            .to_string(),
    ]);
    // Retained observations are host-supplied historical data. Derive changes
    // from the real inspector result to exercise custody and validation, not to
    // claim that the inspector could read a file and its child simultaneously.
    let rehash = |source: &mut Value| {
        for file in source["report"]["files"].as_array_mut().unwrap() {
            let text = file["text"].as_str().unwrap();
            let length = text.len();
            let digest = ContentDigest::of_bytes(text.as_bytes());
            file["byte_length"] = json!(length);
            file["source_digest"] = json!(digest);
            if file["inspection"].is_object() {
                file["inspection"]["source_digest"] = file["source_digest"].clone();
            }
        }
        source["report"]["source_digest"] = json!(ContentDigest::of_bytes(
            &toolkit_package::canonical_json(&source["report"]["files"]).unwrap()
        ));
    };
    let mut changed = observation.clone();
    changed["report"]["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == files[1])
        .unwrap()["text"] = json!("changed without retained hashes\n");
    fs::write(
        fixture.root.join("plugin-source.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    invoke(&save, Some("invalid_plugin_source"));
    rehash(&mut changed);
    fs::write(
        fixture.root.join("plugin-source.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    invoke(&save, Some("immutable_conflict"));

    let mut invalid_namespace = observation.clone();
    let plugin = invalid_namespace["report"]["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|file| file["path"] == "./plugin.json")
        .unwrap();
    let mut manifest: Value = serde_json::from_str(plugin["text"].as_str().unwrap()).unwrap();
    manifest["extensions"]["ai.rkat.toolkit"]["schema_version"] = json!(999);
    plugin["text"] = json!(serde_json::to_string(&manifest).unwrap());
    rehash(&mut invalid_namespace);
    fs::write(
        fixture.root.join("plugin-source.json"),
        serde_json::to_vec(&invalid_namespace).unwrap(),
    )
    .unwrap();
    invoke(&save, Some("invalid_plugin_source"));

    let mut collision = observation;
    collision["report"]["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": "./skills/observed-guide/data", "text": "historically observed text\n",
            "byte_length": 0, "source_digest": "0".repeat(64), "inspection": null
        }));
    rehash(&mut collision);
    fs::write(
        fixture.root.join("plugin-source.json"),
        serde_json::to_vec(&collision).unwrap(),
    )
    .unwrap();
    replace(&mut save, "--owner", "collision-owner");
    let retained = invoke(&save, None);
    let mut export = guidance_args(
        &fixture,
        &retained,
        &json!({
            "plugin_source_digest": retained["recipe"]["plugin_source_digest"],
            "files": [files[0], "./skills/observed-guide/data", files[1]]
        }),
    );
    replace(&mut export, "--owner", "collision-owner");
    invoke(&export, Some("portable_export_refused"));
    assert!(!fixture.root.join("exported").exists());
    assert_eq!(
        invoke(
            &fixture.read(saved["recipe"]["digest"].as_str().unwrap()),
            None
        ),
        before
    );
}
