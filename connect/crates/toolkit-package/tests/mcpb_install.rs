#![cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;
use toolkit_package::local_artifacts::{
    LocalArtifactInputs, LocalArtifactLayout, LocalArtifactPreparer, PreparationLimits,
};
use toolkit_package::*;
use zip::write::SimpleFileOptions;

fn fixture_manifest() -> Value {
    json!({"manifest_version":"0.3","name":"node-server","version":"0.0.0-fixture",
        "description":"Synthetic fixture, no provider or actual Node proof","author":{"name":"Fixture"},
        "server":{"type":"node","entry_point":"server.mjs","mcp_config":{"command":"node",
            "args":["${__dirname}/server.mjs","--toolsets=${user_config.toolsets}"]}},
        "compatibility":{"platforms":["darwin","linux"],"runtimes":{"node":">=22.0.0"}},
        "user_config":{"toolsets":{"type":"string","title":"Toolsets","default":""}}})
}

fn archive(manifest: &[u8], script: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, bytes) in [("manifest.json", manifest), ("server.mjs", script)] {
        writer.start_file(name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

struct Fixture {
    root: TempDir,
    package: IntegrationPackage,
    selection: ActivationManifest,
    config: Vec<u8>,
    artifact: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let package: IntegrationPackage =
            serde_json::from_slice(include_bytes!("fixtures/node-install/package.json")).unwrap();
        let mut selection: ActivationManifest =
            serde_json::from_slice(include_bytes!("fixtures/node-install/activation.json"))
                .unwrap();
        let config = br#"{"toolsets":""}"#.to_vec();
        let artifact = archive(
            &serde_json::to_vec(&fixture_manifest()).unwrap(),
            b"// inert fixture script\n",
        );
        selection.configuration.content_digest = ContentDigest::of_bytes(&config);
        let ComponentSource::Artifact {
            dependencies,
            entrypoint,
            content_digest,
            ..
        } = &mut selection.components[0].source
        else {
            unreachable!()
        };
        *dependencies = ArtifactDependencies::SelfContained;
        *entrypoint = Entrypoint {
            executable: "server.mjs".into(),
            arguments: vec!["--toolsets=".into()],
            python_console_script: None,
        };
        *content_digest = ContentDigest::of_bytes(&artifact);
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
            package,
            selection,
            config,
            artifact,
        };
        fixture.sync();
        fixture.node(b"#!/bin/sh\n[ \"$1\" = --version ] || exit 9\nprintf 'v24.19.0\\n'\n");
        fixture
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn sync(&self) {
        for (name, bytes) in [
            ("package.json", canonical_json(&self.package).unwrap()),
            ("activation.json", canonical_json(&self.selection).unwrap()),
            ("configuration.json", self.config.clone()),
            ("artifact.mcpb", self.artifact.clone()),
        ] {
            std::fs::write(self.path(name), bytes).unwrap();
        }
    }
    fn replace_manifest(&mut self, manifest: Value) {
        self.artifact = archive(&serde_json::to_vec(&manifest).unwrap(), b"// fixture\n");
        let ComponentSource::Artifact { content_digest, .. } =
            &mut self.selection.components[0].source
        else {
            unreachable!()
        };
        *content_digest = ContentDigest::of_bytes(&self.artifact);
        self.sync();
    }
    fn node(&self, bytes: &[u8]) {
        std::fs::write(self.path("node"), bytes).unwrap();
        std::fs::set_permissions(self.path("node"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    fn arguments(&self, action: &str, destination: &str) -> Vec<String> {
        let mut args = vec![action.to_string()];
        for (flag, name) in [
            ("--package", "package.json"),
            ("--selection", "activation.json"),
            ("--config", "configuration.json"),
            ("--node", "node"),
        ] {
            if action == "check" && flag == "--node" {
                continue;
            }
            args.extend([flag.into(), self.path(name).display().to_string()]);
        }
        if action != "verify" {
            args.extend([
                "--artifact".into(),
                self.path("artifact.mcpb").display().to_string(),
            ]);
        }
        args.extend([
            "--selection-sha256".into(),
            ContentDigest::of_bytes(&canonical_json(&self.selection).unwrap()).to_string(),
            "--config-sha256".into(),
            ContentDigest::of_bytes(&self.config).to_string(),
            "--component".into(),
            "node-server".into(),
            "--destination".into(),
            self.path(destination).display().to_string(),
        ]);
        args
    }
    fn call(&self, action: &str, destination: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_toolkit-mcpb-install"))
            .args(self.arguments(action, destination))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", self.root.path())
            .current_dir(self.root.path())
            .output()
            .unwrap()
    }
    fn plan(&self) -> Result<McpbInstallPlan, ValidationError> {
        let selected = self.selection.clone().validate(&self.package)?;
        let temp = tempfile::tempdir().unwrap();
        let owner = LocalArtifactPreparer::new(
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap(),
        );
        let prepared = owner
            .prepare(
                &selected,
                &Id::new("node-server").unwrap(),
                LocalArtifactLayout::Zip,
                LocalArtifactInputs {
                    artifact: std::fs::File::open(self.path("artifact.mcpb")).unwrap(),
                    pinned_dependencies: BTreeMap::new(),
                },
                &PreparationLimits::default(),
            )
            .unwrap();
        McpbInstallPlan::new(
            &self.package,
            &selected,
            &Id::new("node-server").unwrap(),
            &prepared,
            &self.config,
        )
    }
}

fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn actual_cli_installs_reopens_and_restores_without_launching_fixture_script() {
    let fixture = Fixture::new();
    let receipt = success(&fixture.call("install", "first install"));
    assert_eq!(receipt["scope"], "observed_mcpb_software_installation_only");
    assert_eq!(receipt["runtime"]["version"], "v24.19.0");
    let launch = &receipt["mcp_configuration"]["mcpServers"]["fixture-node"];
    assert_eq!(
        launch["args"][0],
        fixture
            .path("first install/prepared/tree/server.mjs")
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(launch["args"][1], "--toolsets=");
    assert_eq!(launch.as_object().unwrap().len(), 2);
    assert_eq!(success(&fixture.call("verify", "first install")), receipt);
    let restored = success(&fixture.call("install", "restored"));
    assert_eq!(restored["plan"], receipt["plan"]);
    assert_ne!(restored["destination"], receipt["destination"]);
    assert!(!fixture.call("install", "first install").status.success());
}

#[test]
fn check_emits_the_same_canonical_plan_without_runtime_or_ready_publication() {
    let fixture = Fixture::new();
    fixture.node(b"#!/bin/sh\nprintf invoked > node-invoked\nexit 9\n");
    let output = fixture.call("check", "checked");
    let plan = success(&output);
    let mut expected = canonical_json(&plan).unwrap();
    expected.push(b'\n');
    assert_eq!(output.stdout, expected);
    assert_eq!(plan, serde_json::to_value(fixture.plan().unwrap()).unwrap());
    assert!(!fixture.path("node-invoked").exists());
    assert!(fixture.path("checked/prepared/evidence.json").is_file());
    assert!(fixture.path("checked/inputs/configuration.json").is_file());
    assert!(!fixture.path("checked/ready").exists());
    assert!(!fixture.path("checked/pending-ready").exists());
    for field in ["runtime", "destination", "mcp_configuration", "scope"] {
        assert!(plan.get(field).is_none(), "{field}");
    }

    fixture.node(b"#!/bin/sh\n[ \"$1\" = --version ] || exit 9\nprintf 'v24.19.0\\n'\n");
    let installed = success(&fixture.call("install", "installed"));
    assert_eq!(installed["plan"], plan);
    assert_eq!(success(&fixture.call("verify", "installed")), installed);
}

#[test]
fn check_destination_is_retained_and_cannot_be_reused_as_an_installation() {
    let fixture = Fixture::new();
    success(&fixture.call("check", "checked"));
    let evidence = std::fs::read(fixture.path("checked/prepared/evidence.json")).unwrap();
    for action in ["check", "install", "verify"] {
        let output = fixture.call(action, "checked");
        assert!(!output.status.success(), "{action}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            std::fs::read(fixture.path("checked/prepared/evidence.json")).unwrap(),
            evidence
        );
        assert!(!fixture.path("checked/ready").exists());
    }
}

#[test]
fn check_refuses_wrong_pins_and_runtime_or_duplicate_arguments_before_writing() {
    let fixture = Fixture::new();
    for case in [
        "selection-pin",
        "configuration-pin",
        "node",
        "duplicate",
        "missing-artifact",
    ] {
        let mut arguments = fixture.arguments("check", "refused");
        match case {
            "selection-pin" | "configuration-pin" => {
                let flag = if case == "selection-pin" {
                    "--selection-sha256"
                } else {
                    "--config-sha256"
                };
                let index = arguments.iter().position(|value| value == flag).unwrap();
                arguments[index + 1] = "0".repeat(64);
            }
            "node" | "duplicate" => {
                let index = arguments
                    .iter()
                    .position(|value| value == "--artifact")
                    .unwrap();
                arguments[index] = if case == "node" { "--node" } else { "--config" }.into();
            }
            _ => {
                let index = arguments
                    .iter()
                    .position(|value| value == "--artifact")
                    .unwrap();
                arguments.drain(index..index + 2);
            }
        }
        let output = Command::new(env!("CARGO_BIN_EXE_toolkit-mcpb-install"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert!(!output.status.success(), "{case}");
        assert!(output.stdout.is_empty(), "{case}");
        assert!(!fixture.path("refused").exists(), "{case}");
    }
}

#[test]
fn check_uses_archive_and_configuration_owners_and_retains_failed_preparations() {
    for case in [
        "sensitive",
        "arguments",
        "missing-config",
        "duplicate-config",
        "archive",
    ] {
        let mut fixture = Fixture::new();
        match case {
            "sensitive" => {
                let mut manifest = fixture_manifest();
                manifest["user_config"]["toolsets"]["sensitive"] = json!(true);
                fixture.replace_manifest(manifest);
            }
            "arguments" => {
                let mut manifest = fixture_manifest();
                manifest["server"]["mcp_config"]["args"][1] = json!("--disagrees");
                fixture.replace_manifest(manifest);
            }
            "missing-config" | "duplicate-config" => {
                fixture.config = if case == "missing-config" {
                    b"{}".to_vec()
                } else {
                    br#"{"toolsets":"private-marker","toolsets":""}"#.to_vec()
                };
                fixture.selection.configuration.content_digest =
                    ContentDigest::of_bytes(&fixture.config);
                fixture.sync();
            }
            _ => std::fs::write(fixture.path("artifact.mcpb"), b"private-marker").unwrap(),
        }
        let output = fixture.call("check", "refused");
        assert!(!output.status.success(), "{case}");
        assert!(output.stdout.is_empty(), "{case}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-marker"));
        assert!(!fixture.path("refused/ready").exists());
        assert!(!fixture.path("refused/pending-ready").exists());
        if ["sensitive", "arguments", "missing-config"].contains(&case) {
            assert!(
                fixture.path("refused/prepared/evidence.json").is_file(),
                "{case}"
            );
        }
    }
}

#[test]
fn a_checked_plan_does_not_bypass_installation_revalidation() {
    let fixture = Fixture::new();
    success(&fixture.call("check", "checked"));
    let checked_payload = std::fs::read(fixture.path("checked/prepared/payload")).unwrap();
    std::fs::write(fixture.path("artifact.mcpb"), b"changed after check").unwrap();
    let output = fixture.call("install", "refused");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!fixture.path("refused/ready").exists());
    assert_eq!(
        std::fs::read(fixture.path("checked/prepared/payload")).unwrap(),
        checked_payload
    );
}

#[test]
fn configuration_and_raw_selection_pins_are_checked_before_destination_creation() {
    let fixture = Fixture::new();
    for flag in ["--selection-sha256", "--config-sha256"] {
        let mut arguments = fixture.arguments("install", "refused");
        let index = arguments.iter().position(|arg| arg == flag).unwrap();
        arguments[index + 1] = "0".repeat(64);
        let output = Command::new(env!("CARGO_BIN_EXE_toolkit-mcpb-install"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!fixture.path("refused").exists());
    }
}

#[test]
fn plan_rejects_manifest_identity_script_and_argument_disagreement() {
    for field in [
        "name",
        "version",
        "entrypoint",
        "script-argv",
        "literal-argv",
    ] {
        let mut fixture = Fixture::new();
        let mut manifest = fixture_manifest();
        match field {
            "name" => manifest["name"] = json!("other"),
            "version" => manifest["version"] = json!("1.0.0"),
            "entrypoint" => manifest["server"]["entry_point"] = json!("other.mjs"),
            "script-argv" => manifest["server"]["mcp_config"]["args"][0] = json!("server.mjs"),
            _ => manifest["server"]["mcp_config"]["args"][1] = json!("--other"),
        }
        fixture.replace_manifest(manifest);
        assert!(fixture.plan().is_err(), "{field}");
    }
}

#[test]
fn profile_refuses_runtime_overrides_env_setup_and_sensitive_configuration() {
    for field in [
        "runtime",
        "range",
        "platform",
        "override",
        "environment",
        "setup",
        "sensitive",
        "array",
        "number",
        "unknown-config",
        "default-type",
    ] {
        let mut fixture = Fixture::new();
        let mut manifest = fixture_manifest();
        match field {
            "runtime" => manifest["server"]["type"] = json!("python"),
            "range" => manifest["compatibility"]["runtimes"]["node"] = json!("^22"),
            "platform" => manifest["compatibility"]["platforms"] = json!(["win32"]),
            "override" => {
                manifest["server"]["mcp_config"]["platform_overrides"] =
                    json!({"darwin":{"command":"other"}})
            }
            "environment" => manifest["server"]["mcp_config"]["env"] = json!({"TOKEN":"forbidden"}),
            "setup" => manifest["install"] = json!({"command":"forbidden"}),
            "sensitive" => {
                manifest["user_config"]["toolsets"]["sensitive"] = json!(true);
                manifest["user_config"]["toolsets"]
                    .as_object_mut()
                    .unwrap()
                    .remove("default");
            }
            "array" => manifest["user_config"]["toolsets"]["multiple"] = json!(true),
            "number" => manifest["user_config"]["toolsets"]["type"] = json!("number"),
            "unknown-config" => manifest["user_config"]["toolsets"]["setup"] = json!(true),
            _ => manifest["user_config"]["toolsets"]["default"] = json!(12),
        }
        fixture.replace_manifest(manifest);
        assert!(fixture.plan().is_err(), "{field}");
    }
}

#[test]
fn substitution_refuses_ambient_unknown_nested_and_path_ambiguity() {
    for argument in [
        "${env.HOME}",
        "${__dirname}/other",
        "${user_config.missing}",
        "${user_config.toolsets",
        "$HOME",
        "$(command)",
    ] {
        let mut fixture = Fixture::new();
        let mut manifest = fixture_manifest();
        manifest["server"]["mcp_config"]["args"][1] = json!(argument);
        fixture.replace_manifest(manifest);
        assert!(fixture.plan().is_err(), "{argument}");
    }
    assert!(strict_mcpb_json(br#"{"toolsets":"a","toolsets":"b"}"#).is_err());
    let plan = Fixture::new().plan().unwrap();
    assert!(plan.check_node_version("v21.9.9").is_err());
    assert!(plan.check_node_version("v22.0.0").is_ok());
    assert!(plan.check_node_version("v25.1.0-rc").is_err());
}

#[test]
fn unsupported_selection_refuses_before_materialization() {
    for mode in ["auth", "locked", "pinned", "frontend-runtime"] {
        let mut fixture = Fixture::new();
        match mode {
            "auth" => {
                let Export::ServiceConnector(export) = &mut fixture.selection.exports[0] else {
                    unreachable!()
                };
                export.authentication = AuthenticationRequirement::Binding {
                    owner: Id::new("owner").unwrap(),
                    requirement: Id::new("credential").unwrap(),
                    credential_audience: Id::new("audience").unwrap(),
                };
            }
            "frontend-runtime" => fixture.selection.components[0].runtime = RuntimeKind::Python,
            _ => {
                let ComponentSource::Artifact { dependencies, .. } =
                    &mut fixture.selection.components[0].source
                else {
                    unreachable!()
                };
                *dependencies = if mode == "pinned" {
                    ArtifactDependencies::PinnedClosure {
                        artifacts: BTreeMap::new(),
                    }
                } else {
                    ArtifactDependencies::Locked {
                        ecosystem: Id::new("npm").unwrap(),
                        lock_digest: ContentDigest::of_bytes(b"lock"),
                    }
                };
            }
        }
        fixture.sync();
        assert!(!fixture.call("install", "refused").status.success());
        assert!(!fixture.path("refused").exists());
    }
}

#[test]
fn archive_tampering_never_publishes_launch_and_verify_does_not_repair() {
    let fixture = Fixture::new();
    std::fs::write(fixture.path("artifact.mcpb"), b"tampered").unwrap();
    assert!(!fixture.call("install", "failed").status.success());
    assert!(!fixture.path("failed/ready").exists());
    assert!(!fixture.call("verify", "failed").status.success());
    assert!(!fixture.path("failed/ready").exists());
}

#[test]
fn failure_after_prepared_publication_remains_visible_and_has_no_implicit_retry() {
    let mut fixture = Fixture::new();
    let mut manifest = fixture_manifest();
    manifest["server"]["mcp_config"]["args"][1] = json!("--disagrees");
    fixture.replace_manifest(manifest);
    assert!(!fixture.call("install", "partial").status.success());
    assert!(fixture.path("partial/prepared/evidence.json").is_file());
    assert!(!fixture.path("partial/ready").exists());
    assert!(!fixture.call("verify", "partial").status.success());
    assert!(!fixture.call("install", "partial").status.success());
}

#[test]
fn verify_refuses_tampered_prepared_payload_launch_receipt_and_runtime() {
    for field in ["payload", "receipt", "runtime", "inputs", "extra"] {
        let fixture = Fixture::new();
        success(&fixture.call("install", "installed"));
        match field {
            "payload" => {
                let p = fixture.path("installed/prepared/tree/server.mjs");
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(p, b"changed").unwrap();
            }
            "receipt" => std::fs::write(fixture.path("installed/ready/mcp.json"), b"{}").unwrap(),
            "runtime" => fixture.node(b"#!/bin/sh\nprintf 'v25.0.0\\n'\n"),
            "inputs" => {
                std::fs::write(fixture.path("installed/inputs/configuration.json"), b"{}").unwrap()
            }
            _ => std::fs::write(fixture.path("installed/extra"), b"unexpected").unwrap(),
        }
        assert!(
            !fixture.call("verify", "installed").status.success(),
            "{field}"
        );
    }
}

#[test]
fn runtime_probe_is_bounded_and_errors_do_not_echo_child_output() {
    for body in [
        b"#!/bin/sh\nprintf 'private-marker\\n'\n".as_slice(),
        b"#!/bin/sh\nwhile :; do printf 'private-marker'; done\n",
        b"#!/bin/sh\nwhile :; do :; done\n",
    ] {
        let fixture = Fixture::new();
        fixture.node(body);
        let started = std::time::Instant::now();
        let output = fixture.call("install", "refused");
        assert!(!output.status.success());
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-marker"));
        assert!(!fixture.path("refused/ready").exists());
    }
}

#[test]
fn symlink_inputs_and_existing_destinations_are_not_followed_or_replaced() {
    let fixture = Fixture::new();
    let config = fixture.path("configuration.json");
    std::fs::rename(&config, fixture.path("real-config")).unwrap();
    std::os::unix::fs::symlink(fixture.path("real-config"), &config).unwrap();
    assert!(!fixture.call("install", "refused").status.success());
    assert!(!fixture.path("refused").exists());
    std::fs::remove_file(config).unwrap();
    fixture.sync();
    std::os::unix::fs::symlink(fixture.root.path(), fixture.path("linked")).unwrap();
    assert!(!fixture.call("install", "linked").status.success());
    assert!(!fixture.path("inputs").exists());
}

#[test]
fn canonical_destination_rejects_non_utf8_symlink_ancestors_before_mutation() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let invalid_argument = std::ffi::OsString::from_vec(vec![b'p', 0xff]);
    let output = Command::new(env!("CARGO_BIN_EXE_toolkit-mcpb-install"))
        .arg(&invalid_argument)
        .env_clear()
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"invalid_arguments\n");
    let hidden = fixture.root.path().join(invalid_argument);
    match std::fs::create_dir(&hidden) {
        Ok(()) => (),
        // Some macOS filesystems cannot represent this fixture. The argv
        // refusal above still runs; Linux also exercises the canonical path.
        Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EILSEQ) => {
            return;
        }
        Err(error) => panic!("cannot create canonical path fixture: {error}"),
    }
    std::os::unix::fs::symlink(&hidden, fixture.path("alias")).unwrap();
    let output = fixture.call("install", "alias/refused");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("utf8_destination_required:"));
    assert!(!hidden.join("refused").exists());
}

#[test]
fn large_valid_argv_receipt_can_be_verified_after_publication() {
    let mut fixture = Fixture::new();
    let arguments = vec!["x".repeat(8000); 80];
    let mut manifest = fixture_manifest();
    let mut mcp_arguments = vec!["${__dirname}/server.mjs".to_string()];
    mcp_arguments.extend(arguments.clone());
    manifest["server"]["mcp_config"]["args"] = json!(mcp_arguments);
    let ComponentSource::Artifact { entrypoint, .. } = &mut fixture.selection.components[0].source
    else {
        unreachable!()
    };
    entrypoint.arguments = arguments;
    fixture.replace_manifest(manifest);
    assert!(canonical_json(&fixture.selection).unwrap().len() < 1024 * 1024);
    let receipt = success(&fixture.call("install", "large"));
    assert!(
        std::fs::metadata(fixture.path("large/ready/installed.json"))
            .unwrap()
            .len()
            > 1024 * 1024
    );
    assert_eq!(success(&fixture.call("verify", "large")), receipt);
}

#[test]
fn explicit_configuration_values_do_not_apply_defaults_or_interpret_substitutions() {
    for config in [
        br#"{}"#.as_slice(),
        br#"{"toolsets":"","extra":""}"#,
        br#"{"toolsets":12}"#,
        br#"{"toolsets":"${env.HOME}"}"#,
        br#"{"toolsets":"a\nb"}"#,
    ] {
        let mut fixture = Fixture::new();
        fixture.config = config.to_vec();
        fixture.selection.configuration.content_digest = ContentDigest::of_bytes(config);
        fixture.sync();
        assert!(fixture.plan().is_err());
    }
    let mut fixture = Fixture::new();
    fixture.selection.configuration.content_digest =
        ContentDigest::of_bytes(b"another explicit selection");
    fixture.sync();
    assert!(!fixture.call("install", "refused").status.success());
    assert!(!fixture.path("refused").exists());
}
