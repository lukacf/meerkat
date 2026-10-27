use serde_json::{Value, json};
use std::collections::BTreeMap;
use toolkit_package::*;

const PACKAGE: &[u8] = include_bytes!("fixtures/python-install/package.json");
const SELECTION: &[u8] = include_bytes!("fixtures/python-install/activation.json");

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn digest(value: &str) -> ContentDigest {
    ContentDigest::of_bytes(value.as_bytes())
}

fn fixture() -> (IntegrationPackage, ActivationManifest) {
    (
        serde_json::from_slice(PACKAGE).unwrap(),
        serde_json::from_slice(SELECTION).unwrap(),
    )
}

// Complete agent-authored inert input. No lifecycle/configuration/policy values
// are synthesized from the full activation fixture.
fn software_fixture(package: &IntegrationPackage) -> Value {
    json!({
        "schema_version":1, "document_kind":"local_software_declaration",
        "package":package.identity, "package_content_digest":package.content_digest().unwrap(),
        "strategy":"python-local",
        "components":[{"id":"python-server","version":"0.0.0-fixture","runtime":"python",
            "source":{"kind":"artifact","location":"https://example.invalid/fixture_server-1.0-py3-none-any.whl","content_digest":"1".repeat(64),
                "dependencies":{"kind":"pinned_closure","artifacts":{"requirements":"2".repeat(64), "dependency_one-1.0-py3-none-any.whl":"3".repeat(64)}},
                "entrypoint":{"executable":"fixture_server/__main__.py","arguments":[]}}}],
        "exports":[{"role":"service_connector","id":"service","components":["python-server"],
            "authentication":{"kind":"none"},"operations":[{"id":"fixture-read",
                "schema_digest":"5".repeat(64),"declared_effect":"read","required_authority":["read"],
                "resources":{"kind":"connection_wide"},
                "result_limits":{"maximum_bytes":4096,"maximum_items":10}}]}],
        "portable_mcp_entries":[{"name":"fixture-python","component":"python-server",
            "operation_tools":{"fixture-read":"actual_read"}}]
    })
}

fn software_plan(package: &IntegrationPackage, value: Value) -> Result<PythonInstallPlan, ValidationError> {
    let declaration: LocalSoftwareDeclaration = serde_json::from_value(value)
        .map_err(|_| ValidationError { path: "fixture".into(), message: "invalid document".into() })?;
    PythonInstallPlan::from_declaration(package, &declaration.validate(package)?, &id("python-server"))
}

fn plan(
    package: &IntegrationPackage,
    selection: ActivationManifest,
) -> Result<PythonInstallPlan, ValidationError> {
    PythonInstallPlan::new(package, &selection.validate(package)?, &id("python-server"))
}

fn closure(selection: &mut ActivationManifest) -> &mut BTreeMap<Id, ContentDigest> {
    let ComponentSource::Artifact {
        dependencies: ArtifactDependencies::PinnedClosure { artifacts },
        ..
    } = &mut selection.components[0].source
    else {
        panic!("artifact fixture")
    };
    artifacts
}

fn entrypoint(selection: &mut ActivationManifest) -> &mut Entrypoint {
    let ComponentSource::Artifact { entrypoint, .. } = &mut selection.components[0].source else {
        panic!("artifact fixture")
    };
    entrypoint
}

#[test]
fn projection_binds_existing_owners_and_derives_only_filename_and_module() {
    let (package, selection) = fixture();
    let validated = selection.clone().validate(&package).unwrap();
    let value = serde_json::to_value(
        PythonInstallPlan::new(&package, &validated, &id("python-server")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["package"], json!(package.identity));
    assert_eq!(
        value["package_content_digest"],
        json!(package.content_digest().unwrap())
    );
    assert_eq!(value["manifest_digest"], json!(validated.digest()));
    assert!(value.get("document_kind").is_none());
    assert!(value.get("declaration_digest").is_none());
    assert_eq!(value["component"], "python-server");
    assert_eq!(value["export"], "service");
    assert_eq!(value["strategy"], "python-local");
    assert_eq!(value["mcp_name"], "fixture-python");
    let source = &serde_json::to_value(&selection).unwrap()["components"][0]["source"];
    assert_eq!(value["artifact_digest"], source["content_digest"]);
    assert_eq!(value["closure"], source["dependencies"]["artifacts"]);
    assert_eq!(value["entrypoint"], source["entrypoint"]);
    assert_eq!(
        value["artifact_filename"],
        "fixture_server-1.0-py3-none-any.whl"
    );
    assert_eq!(value["module"], "fixture_server.__main__");
    assert_eq!(value.as_object().unwrap().len(), 13);
}

#[test]
fn projection_cannot_rebind_validated_package_or_component() {
    let (mut package, selection) = fixture();
    let validated = selection.validate(&package).unwrap();
    assert!(PythonInstallPlan::new(&package, &validated, &id("another-component")).is_err());
    package.description.push_str(" changed");
    assert!(PythonInstallPlan::new(&package, &validated, &id("python-server")).is_err());
}

#[test]
fn shared_selection_gate_refuses_unsupported_compositions() {
    for mutation in [
        "runtime",
        "placement",
        "lifetime",
        "strategy",
        "self-contained",
        "locked",
        "absent-portable",
        "extra-portable",
        "extra-component",
        "extra-export",
        "required-contract",
    ] {
        let (mut package, mut selection) = fixture();
        match mutation {
            "runtime" => selection.components[0].runtime = RuntimeKind::Node,
            "placement" => selection.components[0].placement = id("gcp"),
            "lifetime" => selection.components[0].lifetime = ComponentLifetime::ExternallyManaged,
            "strategy" => {
                package.strategies[0].kind = StrategyKind::NativeAdapter;
                selection.package_content_digest = package.content_digest().unwrap();
            }
            "self-contained" | "locked" => {
                let ComponentSource::Artifact { dependencies, .. } =
                    &mut selection.components[0].source
                else {
                    unreachable!()
                };
                *dependencies = if mutation == "locked" {
                    ArtifactDependencies::Locked {
                        ecosystem: id("pip"),
                        lock_digest: digest("lock"),
                    }
                } else {
                    ArtifactDependencies::SelfContained
                };
            }
            "absent-portable" => selection.portable_mcp_entries.clear(),
            "extra-portable" => selection.portable_mcp_entries.push(PortableMcpEntry {
            environment_secret_slots: None,
                operation_tools: None,
                name: id("second-name"),
                component: id("python-server"),
            }),
            "extra-component" => {
                let mut component = selection.components[0].clone();
                component.id = id("second-component");
                selection.components.push(component);
            }
            "extra-export" => {
                let Export::ServiceConnector(mut export) = selection.exports[0].clone() else {
                    unreachable!()
                };
                export.id = id("second-export");
                selection.exports.push(Export::ServiceConnector(export));
            }
            "required-contract" => selection.required_contracts.push(ContractRequirement {
                name: id("native-owner"),
                version: VersionRequirement {
                    major: 1,
                    minimum_minor: 0,
                },
            }),
            _ => unreachable!(),
        }
        assert!(plan(&package, selection).is_err(), "accepted {mutation}");
    }
}

#[test]
fn python_profile_refuses_frontend_and_event_routes() {
    let (mut package, mut selection) = fixture();
    let Export::ServiceConnector(service) = selection.exports[0].clone() else {
        unreachable!()
    };
    selection.components[0].lifetime = ComponentLifetime::Deployment;
    selection.exports[0] = Export::AgentFrontend(AgentFrontendExport {
        id: service.id,
        components: service.components,
        modalities: [Modality::Text].into(),
        admission_contract: service.enforcement_contract.clone(),
        conversation_contract: service.enforcement_contract.clone(),
        delivery_contract: service.enforcement_contract,
        control_contract: None,
        live_contract: None,
        authentication: AuthenticationRequirement::None,
        diagnostics: service.diagnostics,
        cleanup: service.cleanup,
    });
    package.known_exports[0].role = ExportRole::AgentFrontend;
    selection.package_content_digest = package.content_digest().unwrap();
    let validated = selection.validate(&package).unwrap();
    assert!(PythonInstallPlan::new(&package, &validated, &id("python-server")).is_err());

    let (package, selection) = fixture();
    let mut value = serde_json::to_value(selection).unwrap();
    value["exports"][0]["events"] = json!([{
        "id":"incoming", "payload_schema":digest("event"),
        "authenticated_source_contract":{"name":"fixture/source","version":{"major":1,"minimum_minor":0}},
        "consumer_contract":{"name":"fixture/consumer","version":{"major":1,"minimum_minor":0}},
        "semantics":{
            "identity":"event-id", "ordering":{"kind":"unordered"},
            "maximum_age_ms":1000, "maximum_in_flight":1, "backpressure":"pause_producer",
            "delivery":{"kind":"best_effort", "loss":{"kind":"drop_newest"}, "recovery":{"kind":"next_event"}}
        }
    }]);
    let selection: ActivationManifest = serde_json::from_value(value).unwrap();
    let validated = selection.validate(&package).unwrap();
    assert!(PythonInstallPlan::new(&package, &validated, &id("python-server")).is_err());
}

#[test]
fn module_projection_preserves_explicit_script_and_literal_arguments() {
    for (path, module) in [
        ("main.py", "main"),
        ("_package/sub2/run.py", "_package.sub2.run"),
        ("mcp_server_git/__main__.py", "mcp_server_git.__main__"),
    ] {
        let (package, mut selection) = fixture();
        entrypoint(&mut selection).executable = path.into();
        entrypoint(&mut selection).arguments = vec![
            "$(literal)".into(),
            "--repository".into(),
            "/selected directory".into(),
        ];
        selection.components[0].lifetime = ComponentLifetime::Deployment;
        let value = serde_json::to_value(plan(&package, selection).unwrap()).unwrap();
        assert_eq!(value["module"], module);
        assert_eq!(value["entrypoint"]["executable"], path);
        assert_eq!(
            value["entrypoint"]["arguments"],
            json!(["$(literal)", "--repository", "/selected directory"])
        );
        assert!(value.get("ready").is_none());
        assert!(value.get("authorized").is_none());
    }
}

#[test]
fn explicit_console_script_projects_primary_module_without_inferred_effects() {
    for (path, module) in [("outside_server/__init__.py", "outside_server"),
                           ("outside_server/server.py", "outside_server.server")] {
        let (package, mut value) = fixture();
        let entry = entrypoint(&mut value);
        entry.executable = path.into();
        entry.python_console_script = Some("serve-it".into());
        let selected = serde_json::to_value(plan(&package, value).unwrap()).unwrap();
        assert_eq!(selected["module"], module);
        assert_eq!(selected["entrypoint"]["python_console_script"], "serve-it");
        assert!(selected.get("ready").is_none());
        assert!(selected.get("authorized").is_none());

        let mut declaration = software_fixture(&package);
        let prior = serde_json::to_value(software_plan(&package, declaration.clone()).unwrap()).unwrap();
        declaration["components"][0]["source"]["entrypoint"] = json!({
            "executable":path, "python_console_script":"serve-it", "arguments":["literal;argument"]
        });
        let declared = serde_json::to_value(software_plan(&package, declaration).unwrap()).unwrap();
        assert_eq!(declared["module"], module);
        assert_eq!(declared["declared_service"], prior["declared_service"]);
        assert_ne!(declared["declaration_digest"], prior["declaration_digest"]);
    }
}

#[test]
fn console_script_selection_refuses_invalid_names_runtime_and_root_initializer() {
    for name in ["", "../serve", "/serve", "a/b", "a;b", "a:b", "with space", "x\0y"] {
        let (package, mut value) = fixture();
        entrypoint(&mut value).python_console_script = Some(name.into());
        assert!(plan(&package, value).is_err(), "accepted {name:?}");
    }
    let (package, mut value) = fixture();
    entrypoint(&mut value).python_console_script = Some("serve-it".into());
    entrypoint(&mut value).executable = "__init__.py".into();
    assert!(plan(&package, value).is_err());
    for runtime in [RuntimeKind::Node, RuntimeKind::Native, RuntimeKind::Container] {
        let (package, mut value) = fixture();
        entrypoint(&mut value).python_console_script = Some("serve-it".into());
        value.components[0].runtime = runtime;
        assert!(value.validate(&package).is_err());
    }
    let (package, value) = fixture();
    let selected = serde_json::to_value(plan(&package, value).unwrap()).unwrap();
    assert!(selected["entrypoint"].get("python_console_script").is_none());
}

#[test]
fn module_projection_refuses_non_identifier_paths_and_package_initializers() {
    for path in [
        "package/__init__.py",
        "__init__.py",
        "package.pyc",
        "package/run.PY",
        "two-words/run.py",
        "two.words/run.py",
        "2package/run.py",
        "package/2run.py",
        "package/.py",
        "package/caf\u{e9}.py",
        "package//run.py",
        "package/../run.py",
        "/package/run.py",
        "package\\run.py",
    ] {
        let (package, mut selection) = fixture();
        entrypoint(&mut selection).executable = path.into();
        assert!(plan(&package, selection).is_err(), "accepted {path}");
    }
}

#[test]
fn original_wheel_name_comes_only_from_safe_url_basename() {
    for location in [
        "https://example.invalid/",
        "https://example.invalid/server.tar.gz",
        "https://example.invalid/a%2fb.whl",
        "https://example.invalid/a%20b.whl",
        "https://example.invalid/.hidden.whl",
        "https://example.invalid/a.whl/",
        "https://example.invalid/a.WHL",
        "https://example.invalid/caf%C3%A9.whl",
    ] {
        let (package, mut selection) = fixture();
        let ComponentSource::Artifact {
            location: target, ..
        } = &mut selection.components[0].source
        else {
            unreachable!()
        };
        *target = location.parse().unwrap();
        assert!(plan(&package, selection).is_err(), "accepted {location}");
    }
    let (package, mut selection) = fixture();
    let ComponentSource::Artifact { location, .. } = &mut selection.components[0].source else {
        unreachable!()
    };
    *location = "file:///retained/path/server-2.0-py3-none-any.whl"
        .parse()
        .unwrap();
    assert_eq!(
        serde_json::to_value(plan(&package, selection).unwrap()).unwrap()["artifact_filename"],
        "server-2.0-py3-none-any.whl"
    );
}

#[test]
fn closure_requires_requirements_original_wheel_names_and_no_case_aliases() {
    for mutation in [
        "missing",
        "slash",
        "colon",
        "nonwheel",
        "case-alias",
        "primary-alias",
        "oversize",
    ] {
        let (package, mut selection) = fixture();
        let files = closure(&mut selection);
        match mutation {
            "missing" => {
                files.remove(&id("requirements"));
            }
            "slash" => {
                files.insert(id("nested/archive.whl"), digest("archive"));
            }
            "colon" => {
                files.insert(id("archive:alias.whl"), digest("archive"));
            }
            "nonwheel" => {
                files.insert(id("dependency.tar.gz"), digest("archive"));
            }
            "case-alias" => {
                files.insert(id("Dependency_one-1.0-py3-none-any.whl"), digest("archive"));
            }
            "primary-alias" => {
                files.insert(id("FIXTURE_server-1.0-py3-none-any.whl"), digest("archive"));
            }
            "oversize" => {
                for index in 0..255 {
                    files.insert(
                        id(&format!("wheel_{index}-1-py3-none-any.whl")),
                        digest("archive"),
                    );
                }
            }
            _ => unreachable!(),
        }
        assert!(plan(&package, selection).is_err(), "accepted {mutation}");
    }
    let (package, mut selection) = fixture();
    for index in 0..254 {
        closure(&mut selection).insert(
            id(&format!("wheel_{index}-1-py3-none-any.whl")),
            digest("archive"),
        );
    }
    assert!(
        plan(&package, selection).is_ok(),
        "256 closure entries are supported"
    );
}

#[test]
fn source_and_closure_changes_bind_new_manifest_digest() {
    let (package, mut selection) = fixture();
    let original = serde_json::to_value(plan(&package, selection.clone()).unwrap()).unwrap();
    let ComponentSource::Artifact { content_digest, .. } = &mut selection.components[0].source
    else {
        unreachable!()
    };
    *content_digest = digest("different artifact");
    closure(&mut selection).insert(
        id("dependency_one-1.0-py3-none-any.whl"),
        digest("different closure"),
    );
    let changed = serde_json::to_value(plan(&package, selection).unwrap()).unwrap();
    assert_ne!(original["manifest_digest"], changed["manifest_digest"]);
    assert_ne!(original["artifact_digest"], changed["artifact_digest"]);
    assert_ne!(original["closure"], changed["closure"]);
}

#[test]
fn explicit_raw_names_preserve_declaration_origin_and_legacy_projection() {
    let (package, mut selection) = fixture();
    let legacy = serde_json::to_value(plan(&package, selection.clone()).unwrap()).unwrap();
    assert!(legacy.get("declared_service").is_none());
    let legacy_manifest = serde_json::to_value(&selection).unwrap();
    assert!(legacy_manifest["portable_mcp_entries"][0].get("operation_tools").is_none());
    selection.portable_mcp_entries[0].operation_tools = Some(BTreeMap::from([
        (id("fixture-read"), "read_items_from_server".to_owned()),
    ]));
    let declared = serde_json::to_value(plan(&package, selection.clone()).unwrap()).unwrap();
    assert_eq!(declared["declared_service"], json!({"authentication":{"kind":"none"},
        "operation_tools":{"fixture-read":"read_items_from_server"}}));
    assert_ne!(declared["manifest_digest"], legacy["manifest_digest"]);
    let Export::ServiceConnector(service) = &mut selection.exports[0] else { panic!("service fixture"); };
    service.authentication = AuthenticationRequirement::Binding {
        owner: id("process-owner"), requirement: id("key"), credential_audience: id("service"),
    };
    let binding = serde_json::to_value(plan(&package, selection).unwrap()).unwrap();
    assert_eq!(binding["declared_service"]["authentication"], json!({"kind":"binding",
        "owner":"process-owner","requirement":"key","credential_audience":"service"}));
    assert_ne!(binding["manifest_digest"], declared["manifest_digest"]);
}

#[test]
fn raw_mapping_rejects_duplicate_json_keys_before_last_value_can_win() {
    let (_, selection) = fixture();
    let mut raw = serde_json::to_value(selection).unwrap();
    raw["portable_mcp_entries"][0]["operation_tools"] = json!({"fixture-read":"marker"});
    let bytes = serde_json::to_string(&raw).unwrap().replace(
        "\"fixture-read\":\"marker\"", "\"fixture-read\":\"first_raw\",\"fixture-read\":\"second_raw\"");
    assert!(serde_json::from_str::<ActivationManifest>(&bytes).is_err());
}

#[test]
fn explicit_raw_mapping_cannot_alias_omit_or_add_declared_operations() {
    for mode in ["empty", "extra", "missing", "raw-control", "duplicate"] {
        let (package, mut selection) = fixture();
        let mut names = BTreeMap::from([(id("fixture-read"), "raw_read".to_owned())]);
        match mode {
            "empty" => names.clear(),
            "extra" => { names.insert(id("unselected"), "raw_extra".into()); },
            "missing" => { names.remove(&id("fixture-read")); names.insert(id("unselected"), "raw_read".into()); },
            "raw-control" => { names.insert(id("fixture-read"), "raw\nread".into()); },
            "duplicate" => {
                let Export::ServiceConnector(service) = &mut selection.exports[0] else { panic!("service fixture"); };
                let mut extra = service.operations[0].clone(); extra.id = id("second-read");
                service.operations.push(extra); names.insert(id("second-read"), "raw_read".into());
            },
            _ => unreachable!(),
        }
        selection.portable_mcp_entries[0].operation_tools = Some(names);
        assert!(plan(&package, selection).is_err(), "{mode}");
    }
}

#[cfg(unix)]
mod cli {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::process::{Command, Output};
    use tempfile::TempDir;

    struct Fixture {
        _root: TempDir,
        package: PathBuf,
        selection: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let canonical = root.path().canonicalize().unwrap();
            let package = canonical.join("package.json");
            let selection = canonical.join("activation.json");
            fs::write(&package, PACKAGE).unwrap();
            fs::write(&selection, SELECTION).unwrap();
            Self {
                _root: root,
                package,
                selection,
            }
        }
        fn arguments(&self) -> Vec<String> {
            vec![
                "--package".into(),
                self.package.to_str().unwrap().into(),
                "--selection".into(),
                self.selection.to_str().unwrap().into(),
                "--component".into(),
                "python-server".into(),
            ]
        }
    }
    fn run(arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_toolkit-python-plan"))
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .output()
            .unwrap()
    }
    fn refused(output: Output) -> String {
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error
                .trim()
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        );
        error
    }

    #[test]
    fn declaration_flag_revalidates_exact_software_without_accepting_both_forms() {
        let fixture = Fixture::new();
        let (package, _) = super::fixture();
        let declaration = software_fixture(&package);
        let bytes = serde_json::to_vec_pretty(&declaration).unwrap();
        fs::write(&fixture.selection, &bytes).unwrap();
        let mut args = fixture.arguments();
        args[2] = "--declaration".into();
        let output = run(&args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty());
        assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::to_value(software_plan(&package, declaration.clone()).unwrap()).unwrap());
        assert_eq!(fs::read(&fixture.selection).unwrap(), bytes);
        assert_eq!(refused(run(&fixture.arguments())), "invalid_selection\n");
        let mut both = args.clone();
        both.extend(["--selection".into(), fixture.selection.display().to_string()]);
        assert_eq!(refused(run(&both)), "invalid_arguments\n");
        let mut changed = declaration;
        changed["package_content_digest"] = json!("0".repeat(64));
        fs::write(&fixture.selection, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(refused(run(&args)), "invalid_selection\n");
    }

    #[test]
    fn new_process_revalidates_frozen_input_and_never_mutates_documents() {
        let fixture = Fixture::new();
        let arguments = fixture.arguments();
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let (package, selection) = super::fixture();
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::to_value(plan(&package, selection).unwrap()).unwrap()
        );
        assert_eq!(fs::read(&fixture.package).unwrap(), PACKAGE);
        assert_eq!(fs::read(&fixture.selection).unwrap(), SELECTION);
        let mut changed: Value = serde_json::from_slice(SELECTION).unwrap();
        changed["package_content_digest"] = json!(digest("stale package"));
        fs::write(&fixture.selection, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(refused(run(&arguments)), "invalid_selection\n");
    }

    #[test]
    fn arguments_parser_and_profile_errors_are_value_free() {
        let fixture = Fixture::new();
        for mutation in ["duplicate", "unknown", "relative", "wrong-component"] {
            let mut arguments = fixture.arguments();
            match mutation {
                "duplicate" => arguments[2] = "--package".into(),
                "unknown" => arguments[0] = "--private-sentinel".into(),
                "relative" => arguments[1] = "private-sentinel.json".into(),
                "wrong-component" => arguments[5] = "private-sentinel".into(),
                _ => unreachable!(),
            }
            assert!(!refused(run(&arguments)).contains("sentinel"));
        }
        fs::write(&fixture.package, b"private-sentinel").unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "invalid_package\n");
        fs::write(&fixture.package, PACKAGE).unwrap();
        fs::write(&fixture.selection, b"private-sentinel").unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "invalid_selection\n");
        let (_, mut selection) = super::fixture();
        entrypoint(&mut selection).executable = "private-sentinel.py".into();
        fs::write(&fixture.selection, serde_json::to_vec(&selection).unwrap()).unwrap();
        assert_eq!(
            refused(run(&fixture.arguments())),
            "unsupported_python_selection\n"
        );
    }

    #[test]
    fn input_limits_links_and_nonregular_files_are_refused() {
        let fixture = Fixture::new();
        fs::write(&fixture.package, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "input_too_large\n");
        fs::remove_file(&fixture.package).unwrap();
        symlink(&fixture.selection, &fixture.package).unwrap();
        assert_eq!(
            refused(run(&fixture.arguments())),
            "regular_file_required\n"
        );
        fs::remove_file(&fixture.package).unwrap();
        fs::create_dir(&fixture.package).unwrap();
        assert_eq!(
            refused(run(&fixture.arguments())),
            "regular_file_required\n"
        );
        fs::remove_dir(&fixture.package).unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "input_unavailable\n");
    }
}

#[test]
fn software_declaration_projects_package_bytes_without_a_host_selection() {
    let (mut package, _) = fixture();
    let raw = software_fixture(&package);
    let proof = serde_json::from_value::<LocalSoftwareDeclaration>(raw.clone()).unwrap()
        .validate(&package).unwrap();
    let plan = PythonInstallPlan::from_declaration(&package, &proof, &id("python-server")).unwrap();
    let value = serde_json::to_value(&plan).unwrap();
    assert_eq!(value["document_kind"], "local_software_declaration");
    assert_eq!(value["declaration_digest"], json!(proof.digest()));
    assert_eq!(value["artifact_filename"], "fixture_server-1.0-py3-none-any.whl");
    assert_eq!(value["module"], "fixture_server.__main__");
    assert_eq!(value["declared_service"], json!({"authentication":{"kind":"none"},
        "operation_tools":{"fixture-read":"actual_read"}}));
    for key in ["manifest_digest", "configuration", "policy", "ready", "authorized"] {
        assert!(value.get(key).is_none(), "{key}");
    }
    assert!(PythonInstallPlan::from_declaration(&package, &proof, &id("other")).is_err());
    assert!(NodeInstallPlan::from_declaration(&package, &proof, &id("python-server")).is_err());
    package.description.push_str(" changed");
    assert!(PythonInstallPlan::from_declaration(&package, &proof, &id("python-server")).is_err());

    let (package, _) = fixture();
    let mut binding = raw;
    binding["exports"][0]["authentication"] = json!({"kind":"binding", "owner":"package-process",
        "requirement":"service-key", "credential_audience":"remote-service"});
    let declared = serde_json::to_value(software_plan(&package, binding.clone()).unwrap()).unwrap();
    assert_eq!(declared["declared_service"]["authentication"], binding["exports"][0]["authentication"]);
    assert_ne!(declared["declaration_digest"], value["declaration_digest"]);
    binding["portable_mcp_entries"][0].as_object_mut().unwrap().remove("operation_tools");
    assert!(serde_json::to_value(software_plan(&package, binding).unwrap()).unwrap()
        .get("declared_service").is_none());
}

#[test]
fn software_declaration_cannot_discard_custom_requirements_or_claim_a_host_contract() {
    for mutation in ["known-requirement", "unresolved", "role", "strategy", "resource", "events",
        "configuration", "policy", "lifecycle", "enforcement", "host-service", "dependency",
        "remote", "extra-component", "extra-export", "wrong-component", "wrong-package", "wrong-kind"] {
        let (mut package, _) = fixture();
        let mut value = software_fixture(&package);
        match mutation {
            "known-requirement" => package.strategies[0].known_requirements.push(HostServiceRequirement {
                name:id("custom-host"), version:VersionRequirement { major:1, minimum_minor:0 },
                authority:[id("isolate")].into(), availability:RequirementAvailability::Required,
            }),
            "unresolved" => package.strategies[0].unresolved_details.push("Needs operator-owned resource isolation".into()),
            "role" => package.known_exports[0].role = ExportRole::AgentFrontend,
            "strategy" => package.strategies[0].kind = StrategyKind::NativeAdapter,
            "resource" => value["exports"][0]["operations"][0]["resources"] = json!({"kind":"selected",
                "selector":"one-repository", "enforcement_owner":"custom-owner"}),
            "events" => value["exports"][0]["events"] = json!([]),
            "configuration" | "policy" => value[mutation] = json!({"owner":"invented"}),
            "lifecycle" => value["components"][0]["readiness"] = json!({"owner":"invented","checks":["ready"]}),
            "enforcement" => value["exports"][0]["enforcement_contract"] = json!({"name":"custom-host"}),
            "host-service" => value["components"][0]["host_services"] = json!([]),
            "dependency" => value["components"][0]["dependencies"] = json!([]),
            "remote" => value["components"][0]["runtime"] = json!("remote"),
            "extra-component" => { let extra=value["components"][0].clone(); value["components"].as_array_mut().unwrap().push(extra); },
            "extra-export" => { let extra=value["exports"][0].clone(); value["exports"].as_array_mut().unwrap().push(extra); },
            "wrong-component" => value["portable_mcp_entries"][0]["component"] = json!("unselected"),
            "wrong-package" => value["package"]["name"] = json!("other"),
            "wrong-kind" => value["document_kind"] = json!("activation_manifest"),
            _ => unreachable!(),
        }
        // Requirement refusal must not merely fail due to an outdated package hash.
        value["package_content_digest"] = json!(package.content_digest().unwrap());
        assert!(software_plan(&package, value).is_err(), "accepted {mutation}");
    }
}

#[test]
fn software_declaration_reuses_artifact_operation_and_raw_name_checks() {
    for mutation in ["unsafe-entry", "not-module", "credentials-url", "self-contained", "missing-closure",
        "closure-alias", "no-operations", "duplicate-operation", "zero-bound", "map-missing", "map-extra",
        "map-control", "map-duplicate"] {
        let (package, _) = fixture();
        let mut value = software_fixture(&package);
        match mutation {
            "unsafe-entry" => value["components"][0]["source"]["entrypoint"]["executable"] = json!("../run.py"),
            "not-module" => value["components"][0]["source"]["entrypoint"]["executable"] = json!("run.sh"),
            "credentials-url" => value["components"][0]["source"]["location"] = json!("https://name:secret@example.invalid/a.whl"),
            "self-contained" => value["components"][0]["source"]["dependencies"] = json!({"kind":"self_contained"}),
            "missing-closure" => { value["components"][0]["source"]["dependencies"]["artifacts"].as_object_mut().unwrap().remove("requirements"); },
            "closure-alias" => value["components"][0]["source"]["dependencies"]["artifacts"]["DEPENDENCY_one-1.0-py3-none-any.whl"] = json!("4".repeat(64)),
            "no-operations" => value["exports"][0]["operations"] = json!([]),
            "duplicate-operation" => { let op=value["exports"][0]["operations"][0].clone(); value["exports"][0]["operations"].as_array_mut().unwrap().push(op); },
            "zero-bound" => value["exports"][0]["operations"][0]["result_limits"]["maximum_bytes"] = json!(0),
            "map-missing" => value["portable_mcp_entries"][0]["operation_tools"] = json!({}),
            "map-extra" => value["portable_mcp_entries"][0]["operation_tools"]["extra"] = json!("raw_extra"),
            "map-control" => value["portable_mcp_entries"][0]["operation_tools"]["fixture-read"] = json!("raw\nread"),
            "map-duplicate" => {
                let mut op=value["exports"][0]["operations"][0].clone(); op["id"]=json!("other-read");
                value["exports"][0]["operations"].as_array_mut().unwrap().push(op);
                value["portable_mcp_entries"][0]["operation_tools"]["other-read"]=json!("actual_read");
            },
            _ => unreachable!(),
        }
        assert!(software_plan(&package, value).is_err(), "accepted {mutation}");
    }
    let (package, _) = fixture();
    let raw = serde_json::to_string(&software_fixture(&package)).unwrap().replace(
        "\"fixture-read\":\"actual_read\"", "\"fixture-read\":\"first\",\"fixture-read\":\"second\"");
    assert!(serde_json::from_str::<LocalSoftwareDeclaration>(&raw).is_err());
}

#[test]
fn secret_environment_declarations_bind_both_software_document_forms() {
    let (package, mut selected) = fixture();
    let source: Value = serde_json::from_slice(SELECTION).unwrap();
    assert_eq!(serde_json::to_value(&selected.portable_mcp_entries).unwrap(), source["portable_mcp_entries"]);
    selected.portable_mcp_entries[0].operation_tools = Some(BTreeMap::from([(id("fixture-read"), "actual_read".into())]));
    let Export::ServiceConnector(service) = &mut selected.exports[0] else { panic!("service") };
    service.authentication = AuthenticationRequirement::Binding { owner:id("process-owner"), requirement:id("api-key"), credential_audience:id("service") };
    let before = serde_json::to_value(plan(&package, selected.clone()).unwrap()).unwrap();
    assert!(before["declared_service"].get("environment_secret_slots").is_none());
    selected.portable_mcp_entries[0].environment_secret_slots = Some(serde_json::from_value(json!(["TOKEN", "API_KEY"])).unwrap());
    let after = serde_json::to_value(plan(&package, selected.clone()).unwrap()).unwrap();
    assert_eq!(after["declared_service"]["environment_secret_slots"], json!(["API_KEY", "TOKEN"]));
    assert_ne!(before["manifest_digest"], after["manifest_digest"]);
    assert_eq!(before["entrypoint"], after["entrypoint"]);
    let mut software = software_fixture(&package);
    software["exports"][0]["authentication"] = after["declared_service"]["authentication"].clone();
    let plain_before = serde_json::to_value(software_plan(&package,software.clone()).unwrap()).unwrap();
    software["portable_mcp_entries"][0]["environment_secret_slots"] = json!(["TOKEN", "API_KEY"]);
    let plain_after = serde_json::to_value(software_plan(&package,software).unwrap()).unwrap();
    assert_eq!(plain_after["declared_service"]["environment_secret_slots"], after["declared_service"]["environment_secret_slots"]);
    assert_ne!(plain_before["declaration_digest"], plain_after["declaration_digest"]);
    assert_eq!(plain_before["entrypoint"], plain_after["entrypoint"]);
}

#[test]
fn secret_environment_declarations_reject_ambiguity_and_authentication_downgrades() {
    let (package, selected) = fixture();
    let bad = vec![Value::Null, json!([]), json!(["TOKEN","TOKEN"]), json!(["invalid-name"]),
        json!(["1TOKEN"]), json!(["TOKEN",7]), json!(["TOKEN\n"]), json!(["x".repeat(129)]),
        json!((0..257).map(|index| format!("KEY_{index}")).collect::<Vec<_>>())];
    for slots in bad {
        let mut full = serde_json::to_value(&selected).unwrap();
        full["portable_mcp_entries"][0]["environment_secret_slots"] = slots.clone();
        assert!(serde_json::from_value::<ActivationManifest>(full).is_err());
        let mut plain = software_fixture(&package);
        plain["portable_mcp_entries"][0]["environment_secret_slots"] = slots;
        assert!(software_plan(&package,plain).is_err());
    }
    for no_auth in [true,false] {
        let mut full = selected.clone();
        full.portable_mcp_entries[0].environment_secret_slots = Some(serde_json::from_value(json!(["TOKEN"])).unwrap());
        let mut plain = software_fixture(&package);
        plain["portable_mcp_entries"][0]["environment_secret_slots"] = json!(["TOKEN"]);
        if no_auth {
            full.portable_mcp_entries[0].operation_tools = Some(BTreeMap::from([(id("fixture-read"),"actual_read".into())]));
        } else {
            let auth = AuthenticationRequirement::Binding {owner:id("process"),requirement:id("key"),credential_audience:id("service")};
            let Export::ServiceConnector(service) = &mut full.exports[0] else { panic!("service") };
            service.authentication = auth.clone();
            plain["exports"][0]["authentication"] = serde_json::to_value(auth).unwrap();
            plain["portable_mcp_entries"][0].as_object_mut().unwrap().remove("operation_tools");
        }
        assert!(plan(&package,full).is_err());
        assert!(software_plan(&package,plain).is_err());
    }
}
