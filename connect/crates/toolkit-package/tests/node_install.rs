use serde_json::{Value, json};
use std::collections::BTreeMap;
use toolkit_package::*;

const PACKAGE: &[u8] = include_bytes!("fixtures/node-install/package.json");
const SELECTION: &[u8] = include_bytes!("fixtures/node-install/activation.json");

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
        "strategy":"node-local",
        "components":[{"id":"node-server","version":"0.0.0-fixture","runtime":"node",
            "source":{"kind":"artifact","location":"https://example.invalid/node-server.tgz","content_digest":"1".repeat(64),
                "dependencies":{"kind":"pinned_closure","artifacts":{"package-json":"2".repeat(64), "package-lock":"3".repeat(64), "dependency-archive":"4".repeat(64)}},
                "entrypoint":{"executable":"dist/index.js","arguments":[]}}}],
        "exports":[{"role":"service_connector","id":"service","components":["node-server"],
            "authentication":{"kind":"none"},"operations":[{"id":"fixture-read",
                "schema_digest":"5".repeat(64),"declared_effect":"read","required_authority":["read"],
                "resources":{"kind":"connection_wide"},
                "result_limits":{"maximum_bytes":4096,"maximum_items":10}}]}],
        "portable_mcp_entries":[{"name":"fixture-node","component":"node-server",
            "operation_tools":{"fixture-read":"actual_read"}}]
    })
}

fn software_plan(package: &IntegrationPackage, value: Value) -> Result<NodeInstallPlan, ValidationError> {
    let declaration: LocalSoftwareDeclaration = serde_json::from_value(value)
        .map_err(|_| ValidationError { path: "fixture".into(), message: "invalid document".into() })?;
    NodeInstallPlan::from_declaration(package, &declaration.validate(package)?, &id("node-server"))
}

fn plan(
    package: &IntegrationPackage,
    selection: ActivationManifest,
) -> Result<NodeInstallPlan, ValidationError> {
    NodeInstallPlan::new(package, &selection.validate(package)?, &id("node-server"))
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

#[test]
fn projection_binds_existing_package_manifest_artifact_and_entrypoint_owners() {
    let (package, selection) = fixture();
    let validated = selection.clone().validate(&package).unwrap();
    let plan = NodeInstallPlan::new(&package, &validated, &id("node-server")).unwrap();
    let value = serde_json::to_value(plan).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(
        value["package"],
        serde_json::to_value(&package.identity).unwrap()
    );
    assert_eq!(
        value["package_content_digest"],
        json!(package.content_digest().unwrap())
    );
    assert_eq!(value["manifest_digest"], json!(validated.digest()));
    assert!(value.get("document_kind").is_none());
    assert!(value.get("declaration_digest").is_none());
    assert_eq!(value["component"], "node-server");
    assert_eq!(value["export"], "service");
    assert_eq!(value["strategy"], "node-local");
    assert_eq!(value["mcp_name"], "fixture-node");
    let source = &serde_json::to_value(&selection).unwrap()["components"][0]["source"];
    assert_eq!(value["artifact_digest"], source["content_digest"]);
    assert_eq!(value["closure"], source["dependencies"]["artifacts"]);
    assert_eq!(value["entrypoint"], source["entrypoint"]);
    assert_eq!(value.as_object().unwrap().len(), 11);
}

#[test]
fn projection_cannot_rebind_a_validated_manifest_to_another_package_or_component() {
    let (mut package, selection) = fixture();
    let validated = selection.validate(&package).unwrap();
    assert!(NodeInstallPlan::new(&package, &validated, &id("another-component")).is_err());
    package.description.push_str(" changed");
    assert!(NodeInstallPlan::new(&package, &validated, &id("node-server")).is_err());
}

#[test]
fn projection_refuses_unsupported_runtime_placement_lifetime_strategy_and_dependency_forms() {
    for mutation in [
        "runtime",
        "placement",
        "lifetime",
        "strategy",
        "self-contained",
        "locked",
    ] {
        let (mut package, mut selection) = fixture();
        match mutation {
            "runtime" => selection.components[0].runtime = RuntimeKind::Python,
            "placement" => selection.components[0].placement = id("gcp"),
            "lifetime" => selection.components[0].lifetime = ComponentLifetime::ExternallyManaged,
            "strategy" => {
                package.strategies[0].kind = StrategyKind::NativeAdapter;
                selection.package_content_digest = package.content_digest().unwrap();
            }
            _ => {
                let ComponentSource::Artifact { dependencies, .. } =
                    &mut selection.components[0].source
                else {
                    unreachable!()
                };
                *dependencies = if mutation == "locked" {
                    ArtifactDependencies::Locked {
                        ecosystem: id("npm"),
                        lock_digest: digest("lock"),
                    }
                } else {
                    ArtifactDependencies::SelfContained
                };
            }
        }
        assert!(plan(&package, selection).is_err(), "accepted {mutation}");
    }
}

#[test]
fn projection_uses_existing_portable_independence_checks_and_requires_one_entry() {
    for mutation in [
        "absent",
        "extra",
        "mismatch",
        "required-contract",
        "dependent",
        "extra-component",
        "extra-export",
    ] {
        let (package, mut selection) = fixture();
        match mutation {
            "absent" => selection.portable_mcp_entries.clear(),
            "extra" => selection.portable_mcp_entries.push(PortableMcpEntry {
            environment_secret_slots: None,
                operation_tools: None,
                name: id("another-name"),
                component: id("node-server"),
            }),
            "mismatch" => selection.portable_mcp_entries[0].component = id("another-component"),
            "required-contract" => selection.required_contracts.push(ContractRequirement {
                name: id("native-owner"),
                version: VersionRequirement {
                    major: 1,
                    minimum_minor: 0,
                },
            }),
            "dependent" => {
                selection.components[0].activation_scope = ActivationScope::ToolkitRequired {
                    activation_contract: ContractRequirement {
                        name: id("activation-owner"),
                        version: VersionRequirement {
                            major: 1,
                            minimum_minor: 0,
                        },
                    },
                }
            }
            "extra-component" => {
                let mut component = selection.components[0].clone();
                component.id = id("another-component");
                selection.components.push(component);
                let Export::ServiceConnector(export) = &mut selection.exports[0] else {
                    unreachable!()
                };
                export.components.insert(id("another-component"));
            }
            "extra-export" => {
                let Export::ServiceConnector(mut export) = selection.exports[0].clone() else {
                    unreachable!()
                };
                export.id = id("another-export");
                selection.exports.push(Export::ServiceConnector(export));
            }
            _ => unreachable!(),
        }
        assert!(plan(&package, selection).is_err(), "accepted {mutation}");
    }
}

#[test]
fn projection_refuses_frontend_and_event_routes() {
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
    assert!(NodeInstallPlan::new(&package, &validated, &id("node-server")).is_err());

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
    // Exact event construction is checked by the shared manifest owner first.
    let selection: ActivationManifest = serde_json::from_value(value).unwrap();
    let validated = selection.validate(&package).unwrap();
    assert!(NodeInstallPlan::new(&package, &validated, &id("node-server")).is_err());
}

#[test]
fn closure_requires_exact_project_ids_safe_flat_names_and_bounded_cardinality() {
    for mutation in [
        "missing-json",
        "missing-lock",
        "slash",
        "colon",
        "case-alias",
        "oversize",
    ] {
        let (package, mut selection) = fixture();
        let files = closure(&mut selection);
        match mutation {
            "missing-json" => {
                files.remove(&id("package-json"));
            }
            "missing-lock" => {
                files.remove(&id("package-lock"));
            }
            "slash" => {
                files.insert(id("nested/archive"), digest("archive"));
            }
            "colon" => {
                files.insert(id("archive:alias"), digest("archive"));
            }
            "case-alias" => {
                files.insert(id("Package-Json"), digest("archive"));
            }
            "oversize" => {
                for index in 0..254 {
                    files.insert(id(&format!("archive-{index}")), digest("archive"));
                }
            }
            _ => unreachable!(),
        }
        assert!(plan(&package, selection).is_err(), "accepted {mutation}");
    }
}

#[test]
fn literal_arguments_and_changed_pins_are_bound_without_expansion_or_runtime_claims() {
    let (package, mut selection) = fixture();
    let original = serde_json::to_value(plan(&package, selection.clone()).unwrap()).unwrap();
    let ComponentSource::Artifact {
        entrypoint,
        content_digest,
        ..
    } = &mut selection.components[0].source
    else {
        unreachable!()
    };
    entrypoint.arguments = vec!["$(not-a-command)".into(), "literal argument".into()];
    *content_digest = digest("different artifact");
    closure(&mut selection).insert(id("dependency-archive"), digest("different closure"));
    selection.components[0].lifetime = ComponentLifetime::Deployment;
    let changed = serde_json::to_value(plan(&package, selection).unwrap()).unwrap();
    assert_ne!(changed["manifest_digest"], original["manifest_digest"]);
    assert_ne!(changed["artifact_digest"], original["artifact_digest"]);
    assert_ne!(changed["closure"], original["closure"]);
    assert_eq!(
        changed["entrypoint"]["arguments"],
        json!(["$(not-a-command)", "literal argument"])
    );
    assert!(changed.get("ready").is_none());
    assert!(changed.get("authorized").is_none());
    assert!(changed.get("installed").is_none());
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
                "node-server".into(),
            ]
        }
    }

    fn run(arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_toolkit-node-plan"))
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
    fn new_process_revalidates_files_and_projects_same_owner_without_mutation() {
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

        // A prior successful plan is not cached or accepted as validation.
        let mut changed: Value = serde_json::from_slice(SELECTION).unwrap();
        changed["package_content_digest"] = json!(digest("stale package"));
        fs::write(&fixture.selection, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(refused(run(&arguments)), "invalid_selection\n");
    }

    #[test]
    fn arguments_malformed_documents_limits_and_links_fail_with_value_free_errors() {
        let fixture = Fixture::new();
        for mutation in ["duplicate", "unknown", "relative", "wrong-component"] {
            let mut arguments = fixture.arguments();
            match mutation {
                "duplicate" => arguments[2] = "--package".into(),
                "unknown" => arguments[0] = "--not-a-real-flag-sensitive-value".into(),
                "relative" => arguments[1] = "sensitive-value.json".into(),
                "wrong-component" => arguments[5] = "sensitive-component".into(),
                _ => unreachable!(),
            }
            assert!(!refused(run(&arguments)).contains("sensitive"));
        }
        fs::write(&fixture.selection, b"not json with sensitive contents").unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "invalid_selection\n");
        fs::write(&fixture.selection, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert_eq!(refused(run(&fixture.arguments())), "input_too_large\n");
        fs::remove_file(&fixture.selection).unwrap();
        symlink(&fixture.package, &fixture.selection).unwrap();
        assert_eq!(
            refused(run(&fixture.arguments())),
            "regular_file_required\n"
        );
        fs::remove_file(&fixture.selection).unwrap();
        fs::create_dir(&fixture.selection).unwrap();
        assert_eq!(
            refused(run(&fixture.arguments())),
            "regular_file_required\n"
        );
    }
}

#[test]
fn software_node_plan_preserves_exact_closure_and_excludes_full_activation_identity() {
    let (package, _) = fixture();
    let input = software_fixture(&package);
    let proof = serde_json::from_value::<LocalSoftwareDeclaration>(input.clone()).unwrap()
        .validate(&package).unwrap();
    let value = serde_json::to_value(NodeInstallPlan::from_declaration(&package, &proof, &id("node-server")).unwrap()).unwrap();
    assert_eq!(value["document_kind"], "local_software_declaration");
    assert_eq!(value["declaration_digest"], json!(proof.digest()));
    assert_eq!(value["closure"], input["components"][0]["source"]["dependencies"]["artifacts"]);
    assert_eq!(value["entrypoint"], input["components"][0]["source"]["entrypoint"]);
    assert!(value.get("manifest_digest").is_none());
    assert!(value.get("configuration").is_none());
    assert!(value.get("ready").is_none());
    assert!(PythonInstallPlan::from_declaration(&package, &proof, &id("node-server")).is_err());
    for mutation in ["missing-project", "missing-lock", "alias", "nested", "wrong-runtime"] {
        let mut changed=input.clone();
        match mutation {
            "missing-project" => { changed["components"][0]["source"]["dependencies"]["artifacts"].as_object_mut().unwrap().remove("package-json"); },
            "missing-lock" => { changed["components"][0]["source"]["dependencies"]["artifacts"].as_object_mut().unwrap().remove("package-lock"); },
            "alias" => changed["components"][0]["source"]["dependencies"]["artifacts"]["Package-Lock"]=json!("4".repeat(64)),
            "nested" => changed["components"][0]["source"]["dependencies"]["artifacts"]["dir/archive"]=json!("4".repeat(64)),
            "wrong-runtime" => changed["components"][0]["runtime"]=json!("python"),
            _ => unreachable!(),
        }
        assert!(software_plan(&package,changed).is_err(), "{mutation}");
    }
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
