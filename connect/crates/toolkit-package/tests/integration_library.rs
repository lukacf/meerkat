use std::collections::{BTreeMap, BTreeSet};
use toolkit_package::*;

const PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const MCP: &[u8] = include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn digest(value: &str) -> ContentDigest {
    ContentDigest::of_bytes(value.as_bytes())
}
fn contract() -> ContractRequirement {
    ContractRequirement {
        name: id("fixture/service-policy"),
        version: VersionRequirement {
            major: 1,
            minimum_minor: 0,
        },
    }
}
fn host() -> HostCapabilities {
    HostCapabilities {
        contracts: [(contract().name, ContractVersion { major: 1, minor: 0 })].into(),
        services: BTreeMap::new(),
        runtimes: [RuntimeKind::Remote].into(),
        placements: [id("remote")].into(),
    }
}

// These schemas and owner references are explicit offline fixtures. No test
// claims that a provider accepts them or that credentials/grants are ready.
fn selection(bundle: &IntegrationBundle) -> ActivationManifest {
    selection_for(bundle, &bundle.recipes().recipes[0])
}

fn selection_for(bundle: &IntegrationBundle, recipe: &LibraryRecipe) -> ActivationManifest {
    let RecipePlan::RemoteMcp {
        server, publisher, ..
    } = &recipe.plan
    else {
        panic!("remote fixture")
    };
    let PortableMcpServer::StreamableHttp { url, .. } = &bundle.servers()[server] else {
        panic!("remote fixture")
    };
    ActivationManifest {
        schema_version: 1,
        package: bundle.package().identity.clone(),
        package_content_digest: bundle.package().content_digest().unwrap(),
        strategy: recipe.strategy.clone(),
        configuration: ConfigurationSelection {
            owner: id("fixture/config"),
            reference: id("selected"),
            revision: rev("1"),
            content_digest: digest("config"),
        },
        policy: PolicySelection {
            owner: id("fixture/policy"),
            revision: rev("1"),
            content_digest: digest("policy"),
        },
        components: vec![Component {
            id: id("remote"),
            version: rev("offline-observed-contract"),
            source: ComponentSource::RemoteService {
                endpoint: url.clone(),
                publisher: publisher.clone(),
                configuration_revision: rev("1"),
                observed_contract: digest("offline tools schema"),
                transport: RemoteTransport::McpStreamableHttp,
            },
            runtime: RuntimeKind::Remote,
            placement: id("remote"),
            lifetime: ComponentLifetime::ExternallyManaged,
            activation_scope: ActivationScope::Independent,
            dependencies: vec![],
            required_contracts: vec![],
            host_services: vec![],
            state: vec![],
            readiness: ReadinessContract {
                owner: id("fixture/mcp-owner"),
                checks: [id("observe-current-tools")].into(),
            },
            shutdown: ShutdownContract {
                owner: id("fixture/mcp-owner"),
                operation: id("disconnect"),
                maximum_wait_ms: 1000,
                timeout: ShutdownTimeout::PreserveUncertainty,
            },
            rollback: RollbackContract::OwnerReconciliation {
                owner: id("fixture/mcp-owner"),
                operation: id("reconcile"),
            },
        }],
        exports: vec![Export::ServiceConnector(ServiceConnectorExport {
            id: recipe.export.clone(),
            components: [id("remote")].into(),
            authentication: AuthenticationRequirement::Binding {
                owner: id("fixture/auth"),
                requirement: id("account"),
                credential_audience: publisher.clone(),
            },
            operations: vec![OperationDeclaration {
                id: id("fixture-read"),
                schema_digest: digest("offline read schema"),
                declared_effect: DeclaredEffect::Read,
                required_authority: [id("read")].into(),
                resources: ResourceContract::Selected {
                    selector: id("resource"),
                    enforcement_owner: id("fixture/policy"),
                },
                result_limits: ResultLimits {
                    maximum_bytes: 4096,
                    maximum_items: 10,
                },
            }],
            events: vec![],
            enforcement_contract: contract(),
            diagnostics: id("check-account"),
            cleanup: id("release-binding"),
        })],
        required_contracts: vec![],
        host_services: vec![],
        portable_mcp_entries: vec![PortableMcpEntry {
            environment_secret_slots: None,
            operation_tools: None,
            name: id(server),
            component: id("remote"),
        }],
    }
}

// Test-only conversion of explicit offline fixtures. Production never creates
// an ActivationManifest with invented host references for a remote declaration.
fn remote_declaration(selected: &ActivationManifest) -> RemoteExportDeclaration {
    let mut value = serde_json::to_value(selected).unwrap();
    value.as_object_mut().unwrap().remove("configuration");
    value.as_object_mut().unwrap().remove("policy");
    value["components"][0]["source"].as_object_mut().unwrap().remove("configuration_revision");
    value["selection_kind"] = serde_json::json!(REMOTE_EXPORT_DECLARATION);
    serde_json::from_value(value).unwrap()
}

// Synthetic public endpoint; this does not assert anonymous GitHub access.
fn unauthenticated_documents() -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    let mut plugin: serde_json::Value = serde_json::from_slice(PLUGIN).unwrap();
    let mut mcp: serde_json::Value = serde_json::from_slice(MCP).unwrap();
    let mut recipes: serde_json::Value = serde_json::from_slice(RECIPES).unwrap();
    let source_url = "https://example.invalid/public-mcp-documentation";
    plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0]["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(source_url));
    recipes["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "public-fixture", "url": source_url,
            "observed_sha256": digest("synthetic public endpoint documentation"),
            "upstream": {"kind": "documentation_observation"}, "license": "Synthetic test data"
        }));
    recipes["recipes"][0]["prerequisites"] = serde_json::json!([{
        "kind": "application_binding", "instruction": "The host must admit this endpoint."
    }]);
    recipes["recipes"][0]["plan"]["authentication"] = serde_json::json!({
        "kind": "unauthenticated", "evidence": "public-fixture"
    });
    recipes["recipes"][0]["plan"]["publisher"] = serde_json::json!("fixture-public-provider");
    recipes["recipes"][0]["plan"]["read_only_evidence"] = serde_json::json!("public-fixture");
    mcp["mcpServers"]["github-readonly"]["url"] =
        serde_json::json!("https://example.invalid/public-mcp");
    (plugin, mcp, recipes)
}

#[test]
fn legacy_remote_auth_omission_keeps_prechange_digest_and_requires_binding() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    // Captured by the prechange canonical CLI, not recomputed by this test.
    assert_eq!(
        bundle.digest().as_str(),
        "509e52ffca393859ab08134f4d780e004f2355ef3f27860c4aa85dbcb096a334"
    );
    assert!(matches!(
        &bundle.recipes().recipes[0].plan,
        RecipePlan::RemoteMcp {
            authentication: RemoteAuthentication::Binding,
            ..
        }
    ));
    let serialized = serde_json::to_value(bundle.recipes()).unwrap();
    assert!(
        serialized["recipes"][0]["plan"]
            .get("authentication")
            .is_none()
    );
    let mut explicit: serde_json::Value = serde_json::from_slice(RECIPES).unwrap();
    explicit["recipes"][0]["plan"]["authentication"] = serde_json::json!({"kind": "binding"});
    let equivalent =
        IntegrationBundle::parse(PLUGIN, Some(MCP), &serde_json::to_vec(&explicit).unwrap())
            .unwrap();
    assert_eq!(bundle.digest(), equivalent.digest());
    let mut selected = selection(&bundle);
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.authentication = AuthenticationRequirement::None;
    assert!(bundle.export_portable(bundle.digest(), selected).is_err());
}

#[test]
fn documented_unauthenticated_recipe_requires_matching_none_selection() {
    let (plugin, mcp, recipes) = unauthenticated_documents();
    let bundle = IntegrationBundle::parse(
        &serde_json::to_vec(&plugin).unwrap(),
        Some(&serde_json::to_vec(&mcp).unwrap()),
        &serde_json::to_vec(&recipes).unwrap(),
    )
    .unwrap();
    let mut selected = selection(&bundle);
    assert!(
        bundle
            .export_portable(bundle.digest(), selected.clone())
            .is_err(),
        "public recipe must not select an undeclared credential binding"
    );
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.authentication = AuthenticationRequirement::None;
    assert!(
        bundle
            .validate_selection(bundle.digest(), selected.clone(), &host())
            .is_ok()
    );
    let portable = bundle
        .export_portable(bundle.digest(), selected.clone())
        .unwrap();
    let output: serde_json::Value = serde_json::from_slice(portable.mcp_json().unwrap()).unwrap();
    assert_eq!(
        output["mcpServers"]["github-readonly"]["url"],
        "https://example.invalid/public-mcp"
    );
    assert!(
        output["mcpServers"]["github-readonly"]
            .get("headers")
            .is_none()
    );
    for mutation in ["endpoint", "publisher", "configuration", "write"] {
        let mut candidate = selected.clone();
        match mutation {
            "configuration" => candidate.configuration.revision = rev("changed"),
            "write" => {
                let Export::ServiceConnector(export) = &mut candidate.exports[0] else {
                    unreachable!()
                };
                export.operations[0].declared_effect = DeclaredEffect::Write;
            }
            _ => {
                let ComponentSource::RemoteService {
                    endpoint,
                    publisher,
                    ..
                } = &mut candidate.components[0].source
                else {
                    unreachable!()
                };
                if mutation == "endpoint" {
                    *endpoint = "https://example.invalid/other".parse().unwrap();
                } else {
                    *publisher = id("other");
                }
            }
        }
        assert!(
            bundle.export_portable(bundle.digest(), candidate).is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn anonymous_claim_requires_exact_strategy_source_and_consistent_prerequisites() {
    for mutation in [
        "missing_evidence",
        "unknown_evidence",
        "unlinked_source",
        "client_auth",
        "null",
        "unknown_mode",
        "extra_field",
    ] {
        let (mut plugin, mcp, mut recipes) = unauthenticated_documents();
        match mutation {
            "missing_evidence" => {
                recipes["recipes"][0]["plan"]["authentication"]
                    .as_object_mut()
                    .unwrap()
                    .remove("evidence");
            }
            "unknown_evidence" => {
                recipes["recipes"][0]["plan"]["authentication"]["evidence"] =
                    serde_json::json!("unknown")
            }
            "unlinked_source" => {
                plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0]["sources"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "client_auth" => {
                recipes["recipes"][0]["prerequisites"][0]["kind"] =
                    serde_json::json!("client_authentication")
            }
            "null" => recipes["recipes"][0]["plan"]["authentication"] = serde_json::Value::Null,
            "unknown_mode" => {
                recipes["recipes"][0]["plan"]["authentication"]["kind"] =
                    serde_json::json!("automatic")
            }
            "extra_field" => {
                recipes["recipes"][0]["plan"]["authentication"]["token"] =
                    serde_json::json!("forbidden")
            }
            _ => unreachable!(),
        }
        assert!(
            IntegrationBundle::parse(
                &serde_json::to_vec(&plugin).unwrap(),
                Some(&serde_json::to_vec(&mcp).unwrap()),
                &serde_json::to_vec(&recipes).unwrap()
            )
            .is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn portable_export_reuses_exact_recipe_selection_checks() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let selected = selection(&bundle);
    assert!(
        bundle
            .export_portable(bundle.digest(), selected.clone())
            .is_ok()
    );
    assert!(matches!(
        bundle.export_portable(&digest("stale bundle"), selected.clone()),
        Err(LibraryPortableExportError::Selection(
            LibrarySelectionError::StaleBundle
        ))
    ));
    for mutation in [
        "package",
        "endpoint",
        "transport",
        "publisher",
        "configuration",
        "export",
        "strategy",
        "write",
    ] {
        let mut candidate = selected.clone();
        match mutation {
            "package" => candidate.package_content_digest = digest("another package"),
            "configuration" => candidate.configuration.revision = rev("another revision"),
            "strategy" => candidate.strategy = id("not-selected"),
            "export" | "write" => {
                let Export::ServiceConnector(export) = &mut candidate.exports[0] else {
                    unreachable!()
                };
                if mutation == "export" {
                    export.id = id("another-export");
                } else {
                    export.operations[0].declared_effect = DeclaredEffect::Write;
                }
            }
            _ => {
                let ComponentSource::RemoteService {
                    endpoint,
                    publisher,
                    transport,
                    ..
                } = &mut candidate.components[0].source
                else {
                    unreachable!()
                };
                match mutation {
                    "endpoint" => *endpoint = "https://example.org/another-mcp".parse().unwrap(),
                    "transport" => *transport = RemoteTransport::HttpApi,
                    "publisher" => *publisher = id("another-publisher"),
                    _ => unreachable!(),
                }
            }
        }
        assert!(
            matches!(
                bundle.export_portable(bundle.digest(), candidate),
                Err(LibraryPortableExportError::Selection(
                    LibrarySelectionError::Invalid(_)
                ))
            ),
            "accepted {mutation}"
        );
    }
    // Export is inert. It does not synthesize a compatible host or verify the
    // operation declarations against a provider's actual current tool list.
    assert!(matches!(
        bundle.validate_selection(
            bundle.digest(),
            selected.clone(),
            &HostCapabilities::default()
        ),
        Err(LibrarySelectionError::HostIncompatible(_))
    ));
    let mut another_operation = selected.clone();
    let Export::ServiceConnector(export) = &mut another_operation.exports[0] else {
        unreachable!()
    };
    let mut operation = export.operations[0].clone();
    operation.id = id("another-unverified-read");
    export.operations.push(operation);
    assert_eq!(
        bundle
            .export_portable(bundle.digest(), selected)
            .unwrap()
            .mcp_json(),
        bundle
            .export_portable(bundle.digest(), another_operation)
            .unwrap()
            .mcp_json(),
        "operation declarations must not be mistaken for a portable filter"
    );
}

#[test]
fn portable_export_refuses_known_host_requirements_whether_omitted_or_declared() {
    let requirement = HostServiceRequirement {
        name: id("fixture/required-host-policy"),
        version: VersionRequirement {
            major: 1,
            minimum_minor: 0,
        },
        authority: [id("enforce")].into(),
        availability: RequirementAvailability::Required,
    };
    let mut plugin: serde_json::Value = serde_json::from_slice(PLUGIN).unwrap();
    plugin["extensions"][TOOLKIT_NAMESPACE]["strategies"][0]["known_requirements"] =
        serde_json::json!([requirement]);
    let bundle =
        IntegrationBundle::parse(&serde_json::to_vec(&plugin).unwrap(), Some(MCP), RECIPES)
            .unwrap();
    for declare in [false, true] {
        let mut selected = selection(&bundle);
        if declare {
            selected.host_services.push(requirement.clone());
        }
        assert!(bundle.export_portable_declaration(bundle.digest(), remote_declaration(&selected)).is_err());
        assert!(
            matches!(
                bundle.export_portable(bundle.digest(), selected),
                Err(LibraryPortableExportError::Selection(
                    LibrarySelectionError::Invalid(_)
                ))
            ),
            "accepted required host service with declare={declare}"
        );
    }
}

#[test]
fn portable_export_preserves_lifecycle_contract_and_state_refusals() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    for mutation in [
        "activation",
        "component-contract",
        "manifest-contract",
        "host-service",
        "state",
        "dependency",
    ] {
        let mut selected = selection(&bundle);
        match mutation {
            "activation" => {
                selected.components[0].activation_scope = ActivationScope::ToolkitRequired {
                    activation_contract: contract(),
                }
            }
            "component-contract" => selected.components[0].required_contracts.push(contract()),
            "manifest-contract" => selected.required_contracts.push(contract()),
            "host-service" => selected.components[0]
                .host_services
                .push(HostServiceRequirement {
                    name: id("fixture/optional-host-service"),
                    version: VersionRequirement {
                        major: 1,
                        minimum_minor: 0,
                    },
                    authority: BTreeSet::new(),
                    availability: RequirementAvailability::Optional {
                        degraded_behavior: "No host metrics".into(),
                    },
                }),
            "state" => selected.components[0].state.push(StateContract {
                namespace: id("fixture/state"),
                owner: id("fixture/lifecycle"),
                format: rev("1"),
                migrations: vec![],
            }),
            "dependency" => selected.components[0]
                .dependencies
                .push(ComponentDependency {
                    component: id("absent-dependency"),
                    selected_version: rev("1"),
                }),
            _ => unreachable!(),
        }
        assert!(bundle.export_portable_declaration(bundle.digest(), remote_declaration(&selected)).is_err());
        assert!(
            matches!(
                bundle.export_portable(bundle.digest(), selected),
                Err(LibraryPortableExportError::Selection(
                    LibrarySelectionError::Invalid(_)
                ))
            ),
            "accepted {mutation}"
        );
    }
}

#[test]
fn portable_export_requires_exact_nonempty_declared_server_names() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let selected = selection(&bundle);
    for omitted in [false, true] {
        let mut value = serde_json::to_value(&selected).unwrap();
        if omitted {
            value
                .as_object_mut()
                .unwrap()
                .remove("portable_mcp_entries");
        } else {
            value["portable_mcp_entries"] = serde_json::json!([]);
        }
        assert!(matches!(
            bundle.export_portable(bundle.digest(), serde_json::from_value(value).unwrap()),
            Err(LibraryPortableExportError::Export(
                PortableExportError::Unsupported {
                    reason: UnsupportedPortableExport::UndeclaredEntry,
                    ..
                }
            ))
        ));
    }
    let mut extra = selected.clone();
    extra.portable_mcp_entries.push(PortableMcpEntry {
            environment_secret_slots: None,
        operation_tools: None,
        name: id("unselected-alias"),
        component: id("remote"),
    });
    assert!(matches!(
        bundle.export_portable(bundle.digest(), extra),
        Err(LibraryPortableExportError::Export(
            PortableExportError::Unsupported {
                reason: UnsupportedPortableExport::MissingDeclaredEntry,
                ..
            }
        ))
    ));
    let mut wrong_component = selected;
    wrong_component.portable_mcp_entries[0].component = id("absent");
    assert!(matches!(
        bundle.export_portable(bundle.digest(), wrong_component),
        Err(LibraryPortableExportError::Selection(
            LibrarySelectionError::Invalid(_)
        ))
    ));
}

#[test]
fn portable_export_selects_one_strategy_even_when_endpoints_are_shared() {
    for same_endpoint in [false, true] {
        let mut plugin: serde_json::Value = serde_json::from_slice(PLUGIN).unwrap();
        let mut recipes: serde_json::Value = serde_json::from_slice(RECIPES).unwrap();
        let mut mcp: serde_json::Value = serde_json::from_slice(MCP).unwrap();
        let mut alternative = plugin["extensions"][TOOLKIT_NAMESPACE]["strategies"][0].clone();
        alternative["id"] = serde_json::json!("alternative");
        plugin["extensions"][TOOLKIT_NAMESPACE]["strategies"]
            .as_array_mut()
            .unwrap()
            .push(alternative);
        let mut recipe = recipes["recipes"][0].clone();
        recipe["strategy"] = serde_json::json!("alternative");
        recipe["plan"]["server"] = serde_json::json!("alternative-server");
        recipes["recipes"].as_array_mut().unwrap().push(recipe);
        mcp["mcpServers"]["alternative-server"] = mcp["mcpServers"]["github-readonly"].clone();
        if !same_endpoint {
            mcp["mcpServers"]["alternative-server"]["url"] =
                serde_json::json!("https://example.org/selected-mcp");
        }
        let bundle = IntegrationBundle::parse(
            &serde_json::to_vec(&plugin).unwrap(),
            Some(&serde_json::to_vec(&mcp).unwrap()),
            &serde_json::to_vec(&recipes).unwrap(),
        )
        .unwrap();
        for recipe in &bundle.recipes().recipes {
            let RecipePlan::RemoteMcp { server, .. } = &recipe.plan else {
                unreachable!()
            };
            let selected = selection_for(&bundle, recipe);
            let exported = bundle
                .export_portable(bundle.digest(), selected.clone())
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_slice(exported.mcp_json().unwrap()).unwrap();
            assert_eq!(value["mcpServers"].as_object().unwrap().len(), 1);
            assert_eq!(value["mcpServers"][server], mcp["mcpServers"][server]);
            let mut wrong_name = selected;
            wrong_name.portable_mcp_entries[0].name = id(if server == "github-readonly" {
                "alternative-server"
            } else {
                "github-readonly"
            });
            assert!(bundle.export_portable(bundle.digest(), wrong_name).is_err());
        }
    }
}

#[test]
fn portable_export_does_not_turn_guidance_into_an_executable_choice() {
    let remote = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let guidance = IntegrationBundle::parse(
        include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/plugin.json"),
        None,
        include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/recipes.json"),
    )
    .unwrap();
    let mut selected = selection(&remote);
    selected.package = guidance.package().identity.clone();
    selected.package_content_digest = guidance.package().content_digest().unwrap();
    selected.strategy = guidance.recipes().recipes[0].strategy.clone();
    assert!(matches!(guidance.export_portable_declaration(guidance.digest(), remote_declaration(&selected)),
        Err(LibraryPortableExportError::Selection(LibrarySelectionError::GuidanceOnly))));
    selected.components.clear(); selected.exports.clear();
    assert!(matches!(
        guidance.export_portable(guidance.digest(), selected),
        Err(LibraryPortableExportError::Selection(
            LibrarySelectionError::GuidanceOnly
        ))
    ));
}

#[test]
fn retained_packages_load_and_remote_recipes_use_existing_manifest_contracts() {
    let library = bundled_integration_library().unwrap();
    assert_eq!(library.entries().count(), 14);
    assert_eq!(library.catalog().entries().count(), 14);
    let identities: BTreeSet<_> = library
        .entries()
        .map(|bundle| {
            let identity = &bundle.package().identity;
            (identity.publisher.clone(), identity.name.clone())
        })
        .collect();
    assert_eq!(identities.len(), 11);
    let mut remote = 0;
    let mut guidance = 0;
    let mut frontend = 0;
    for bundle in library.entries() {
        assert!(
            library
                .get(&bundle.package().identity, bundle.digest())
                .is_some()
        );
        assert!(
            library
                .get(&bundle.package().identity, &digest("wrong"))
                .is_none()
        );
        assert!(
            bundle
                .package()
                .assessments
                .iter()
                .all(|a| a.status == SupportStatus::Experimental)
        );
        if bundle
            .recipes()
            .recipes
            .iter()
            .all(|recipe| matches!(&recipe.plan, RecipePlan::GuidanceOnly { .. }))
        {
            assert!(bundle.servers().is_empty());
        }
        for recipe in &bundle.recipes().recipes {
            if recipe.role == ExportRole::AgentFrontend {
                frontend += 1;
            }
            match &recipe.plan {
                RecipePlan::RemoteMcp { .. } => {
                    remote += 1;
                    let manifest = bundle
                        .validate_selection(bundle.digest(), selection_for(bundle, recipe), &host())
                        .unwrap();
                    assert_eq!(manifest.manifest().package, bundle.package().identity);
                    assert_eq!(manifest.initialization_order(), &[id("remote")]);
                }
                RecipePlan::GuidanceOnly { blockers } => {
                    guidance += 1;
                    assert!(!blockers.is_empty());
                    let remote_bundle =
                        IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
                    let mut manifest = selection(&remote_bundle);
                    manifest.package = bundle.package().identity.clone();
                    manifest.package_content_digest = bundle.package().content_digest().unwrap();
                    manifest.strategy = recipe.strategy.clone();
                    assert!(matches!(
                        bundle.validate_selection(bundle.digest(), manifest, &host()),
                        Err(LibrarySelectionError::GuidanceOnly)
                    ));
                }
            }
        }
    }
    assert_eq!((remote, guidance, frontend), (6, 11, 1));
}

#[test]
fn unavailable_host_capabilities_and_stale_choices_are_rejected() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    for missing in ["runtime", "placement", "policy"] {
        let mut host = host();
        match missing {
            "runtime" => host.runtimes.clear(),
            "placement" => host.placements.clear(),
            _ => host.contracts.clear(),
        }
        match bundle.validate_selection(bundle.digest(), selection(&bundle), &host) {
            Err(LibrarySelectionError::HostIncompatible(report)) => {
                assert!(!report.required_gaps.is_empty())
            }
            result => panic!("missing {missing} was not rejected: {result:?}"),
        }
    }
    assert!(matches!(
        bundle.validate_selection(&digest("stale"), selection(&bundle), &host()),
        Err(LibrarySelectionError::StaleBundle)
    ));
    for mutate in [
        "endpoint",
        "publisher",
        "config",
        "package",
        "export",
        "write",
        "auth",
        "frontend",
    ] {
        let mut manifest = selection(&bundle);
        match mutate {
            "endpoint" => {
                if let ComponentSource::RemoteService { endpoint, .. } =
                    &mut manifest.components[0].source
                {
                    *endpoint = "https://example.org/mcp".parse().unwrap();
                }
            }
            "publisher" => {
                if let ComponentSource::RemoteService { publisher, .. } =
                    &mut manifest.components[0].source
                {
                    *publisher = id("other");
                }
            }
            "config" => manifest.configuration.revision = rev("changed"),
            "package" => manifest.package_content_digest = digest("changed"),
            "export" => {
                if let Export::ServiceConnector(e) = &mut manifest.exports[0] {
                    e.id = id("other");
                }
            }
            "write" => {
                if let Export::ServiceConnector(e) = &mut manifest.exports[0] {
                    e.operations[0].declared_effect = DeclaredEffect::Write;
                }
            }
            "auth" => {
                if let Export::ServiceConnector(e) = &mut manifest.exports[0] {
                    e.authentication = AuthenticationRequirement::None;
                }
            }
            "frontend" => {
                manifest.exports[0] = Export::AgentFrontend(AgentFrontendExport {
                    id: id("bot"),
                    components: [id("remote")].into(),
                    modalities: [Modality::Text].into(),
                    admission_contract: contract(),
                    conversation_contract: contract(),
                    delivery_contract: contract(),
                    control_contract: None,
                    live_contract: None,
                    authentication: AuthenticationRequirement::None,
                    diagnostics: id("check"),
                    cleanup: id("stop"),
                })
            }
            _ => unreachable!(),
        }
        assert!(
            bundle
                .validate_selection(bundle.digest(), manifest, &host())
                .is_err(),
            "accepted {mutate}"
        );
    }
}

#[test]
fn retained_imports_refuse_skipped_entries_and_unrepresented_authority() {
    for field in [
        "invalid_server",
        "extra_server",
        "inline_auth",
        "frontend",
        "missing_evidence",
        "missing_source",
        "extra_recipe",
        "wrong_version",
    ] {
        let mut plugin: serde_json::Value = serde_json::from_slice(PLUGIN).unwrap();
        let mut mcp: serde_json::Value = serde_json::from_slice(MCP).unwrap();
        let mut recipes: serde_json::Value = serde_json::from_slice(RECIPES).unwrap();
        match field {
            "invalid_server" => {
                mcp["mcpServers"]["broken"] = serde_json::json!({"type":"unsupported"})
            }
            "extra_server" => {
                mcp["mcpServers"]["extra"] = mcp["mcpServers"]["github-readonly"].clone()
            }
            "inline_auth" => {
                mcp["mcpServers"]["github-readonly"]["headers"] =
                    serde_json::json!({"Authorization":"fixture-not-a-secret"})
            }
            "frontend" => recipes["recipes"][0]["role"] = serde_json::json!("agent_frontend"),
            "missing_evidence" => {
                recipes["recipes"][0]["plan"]["read_only_evidence"] = serde_json::Value::Null
            }
            "missing_source" => recipes["sources"] = serde_json::json!([]),
            "extra_recipe" => {
                let recipe = recipes["recipes"][0].clone();
                recipes["recipes"].as_array_mut().unwrap().push(recipe);
            }
            "wrong_version" => plugin["version"] = serde_json::json!("9.9.9"),
            _ => unreachable!(),
        }
        assert!(
            IntegrationBundle::parse(
                &serde_json::to_vec(&plugin).unwrap(),
                Some(&serde_json::to_vec(&mcp).unwrap()),
                &serde_json::to_vec(&recipes).unwrap()
            )
            .is_err(),
            "accepted {field}"
        );
    }
}

#[test]
fn prerequisites_and_endpoint_changes_change_bundle_identity_even_when_metadata_is_unchanged() {
    let original = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let mut recipe: serde_json::Value = serde_json::from_slice(RECIPES).unwrap();
    recipe["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("Changed host prerequisites");
    let changed =
        IntegrationBundle::parse(PLUGIN, Some(MCP), &serde_json::to_vec(&recipe).unwrap()).unwrap();
    assert_eq!(original.package(), changed.package());
    assert_ne!(original.digest(), changed.digest());
    let mut mcp: serde_json::Value = serde_json::from_slice(MCP).unwrap();
    mcp["mcpServers"]["github-readonly"]["url"] = serde_json::json!("https://example.org/mcp");
    let changed_endpoint =
        IntegrationBundle::parse(PLUGIN, Some(&serde_json::to_vec(&mcp).unwrap()), RECIPES)
            .unwrap();
    assert_eq!(original.package(), changed_endpoint.package());
    assert_ne!(original.digest(), changed_endpoint.digest());
    let mut library = IntegrationLibrary::new(CatalogOrigin::Curated {
        catalog: id("test/library"),
        revision: rev("1"),
    });
    library.insert(original.clone()).unwrap();
    library.insert(original).unwrap();
    assert!(matches!(
        library.insert(changed_endpoint),
        Err(IntegrationLibraryError::ImmutableConflict)
    ));
    assert!(matches!(
        library.insert(changed),
        Err(IntegrationLibraryError::ImmutableConflict)
    ));
}

const GMAIL_OLD_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/gmail-api-setup/0.1.0/plugin.json");
const GMAIL_OLD_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/gmail-api-setup/0.1.0/recipes.json");
const GMAIL_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/gmail-api-setup/0.2.0/plugin.json");
const GMAIL_MCP: &[u8] =
    include_bytes!("../integrations/connectors/gmail-api-setup/0.2.0/mcp.json");
const GMAIL_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/gmail-api-setup/0.2.0/recipes.json");

#[test]
fn gmail_versions_remain_distinct_and_historical_selection_does_not_upgrade() {
    assert_eq!(
        ContentDigest::of_bytes(GMAIL_OLD_PLUGIN).as_str(),
        "bf56d1b0e9d2d0bc18ad49c8849c26fe381f4efa185be00ca543ddea32a2374b"
    );
    assert_eq!(
        ContentDigest::of_bytes(GMAIL_OLD_RECIPES).as_str(),
        "4163c8956e663c8eea633bf6c3153fb7bb71206c94860968d6a3ad9782d7e280"
    );
    let old = IntegrationBundle::parse(GMAIL_OLD_PLUGIN, None, GMAIL_OLD_RECIPES).unwrap();
    let current = IntegrationBundle::parse(GMAIL_PLUGIN, Some(GMAIL_MCP), GMAIL_RECIPES).unwrap();
    let library = bundled_integration_library().unwrap();
    assert_eq!(
        old.package().identity.publisher,
        current.package().identity.publisher
    );
    assert_eq!(old.package().identity.name, current.package().identity.name);
    assert_eq!(old.package().identity.version, rev("0.1.0"));
    assert_eq!(current.package().identity.version, rev("0.2.0"));
    assert_ne!(old.digest(), current.digest());
    for bundle in [&old, &current] {
        assert!(
            library
                .get(&bundle.package().identity, bundle.digest())
                .is_some()
        );
        assert_eq!(
            bundle.package().known_exports[0].id,
            id("gmail-api-setup.service")
        );
        assert_eq!(
            bundle.package().known_exports[0].role,
            ExportRole::ServiceConnector
        );
    }
    assert!(
        library
            .get(&old.package().identity, current.digest())
            .is_none()
    );
    assert!(
        library
            .get(&current.package().identity, old.digest())
            .is_none()
    );
    let mut historical = selection(&current);
    historical.package = old.package().identity.clone();
    historical.package_content_digest = old.package().content_digest().unwrap();
    historical.strategy = id("setup");
    assert!(matches!(
        old.validate_selection(old.digest(), historical, &host()),
        Err(LibrarySelectionError::GuidanceOnly)
    ));
}

#[test]
fn gmail_hosted_selection_and_rest_guidance_are_explicit_and_never_substituted() {
    let bundle = IntegrationBundle::parse(GMAIL_PLUGIN, Some(GMAIL_MCP), GMAIL_RECIPES).unwrap();
    assert_eq!(
        bundle
            .package()
            .strategies
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [id("hosted-mcp-preview"), id("toolkit-rest")]
    );
    assert_eq!(bundle.package().strategies[0].kind, StrategyKind::RemoteMcp);
    assert_eq!(
        bundle.package().strategies[1].kind,
        StrategyKind::ConfigurationRecipe
    );
    assert_eq!(bundle.recipes().recipes.len(), 2);
    assert_eq!(bundle.servers().len(), 1);
    let PortableMcpServer::StreamableHttp { url, headers } =
        &bundle.servers()["gmail-hosted-preview"]
    else {
        panic!("Gmail preview must retain the documented transport")
    };
    assert_eq!(url.as_str(), "https://gmailmcp.googleapis.com/mcp/v1");
    assert!(headers.is_empty());
    assert!(matches!(
        &bundle.recipes().recipes[0].plan,
        RecipePlan::RemoteMcp {
            read_only: false,
            read_only_evidence: None,
            ..
        }
    ));
    let selected = selection(&bundle);
    assert!(
        bundle
            .validate_selection(bundle.digest(), selected.clone(), &host())
            .is_ok()
    );
    for mutation in ["endpoint", "auth", "host"] {
        let mut selected = selected.clone();
        let mut capabilities = host();
        match mutation {
            "endpoint" => {
                if let ComponentSource::RemoteService { endpoint, .. } =
                    &mut selected.components[0].source
                {
                    *endpoint = "https://gmail.googleapis.com/".parse().unwrap();
                }
            }
            "auth" => {
                if let Export::ServiceConnector(export) = &mut selected.exports[0] {
                    export.authentication = AuthenticationRequirement::None;
                }
            }
            "host" => capabilities.runtimes.clear(),
            _ => unreachable!(),
        }
        assert!(
            bundle
                .validate_selection(bundle.digest(), selected, &capabilities)
                .is_err()
        );
    }
    let mut rest = selected.clone();
    rest.strategy = id("toolkit-rest");
    assert!(matches!(
        bundle.validate_selection(bundle.digest(), rest, &host()),
        Err(LibrarySelectionError::GuidanceOnly)
    ));
    let mut absent = selected;
    absent.strategy = id("send-automatically");
    assert!(
        bundle
            .validate_selection(bundle.digest(), absent, &host())
            .is_err()
    );
}

#[test]
fn gmail_changed_current_prerequisites_cannot_replace_either_retained_version() {
    let mut library = bundled_integration_library().unwrap();
    let current = IntegrationBundle::parse(GMAIL_PLUGIN, Some(GMAIL_MCP), GMAIL_RECIPES).unwrap();
    let mut recipes: serde_json::Value = serde_json::from_slice(GMAIL_RECIPES).unwrap();
    recipes["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("Changed preview eligibility");
    let changed = IntegrationBundle::parse(
        GMAIL_PLUGIN,
        Some(GMAIL_MCP),
        &serde_json::to_vec(&recipes).unwrap(),
    )
    .unwrap();
    assert_eq!(current.package(), changed.package());
    assert_ne!(current.digest(), changed.digest());
    assert!(matches!(
        library.insert(changed),
        Err(IntegrationLibraryError::ImmutableConflict)
    ));
    assert!(
        library
            .get(&current.package().identity, current.digest())
            .is_some()
    );
    let old = IntegrationBundle::parse(GMAIL_OLD_PLUGIN, None, GMAIL_OLD_RECIPES).unwrap();
    assert!(library.get(&old.package().identity, old.digest()).is_some());
    assert_eq!(library.entries().count(), 14);
}

const SLACK_OLD_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/slack-mcp-setup/0.1.0/plugin.json");
const SLACK_OLD_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/slack-mcp-setup/0.1.0/recipes.json");
const SLACK_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/slack-mcp-setup/0.2.0/plugin.json");
const SLACK_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/slack-mcp-setup/0.2.0/recipes.json");

#[test]
fn slack_versions_preserve_exact_old_bytes_and_service_identity() {
    assert_eq!(
        ContentDigest::of_bytes(SLACK_OLD_PLUGIN).as_str(),
        "eb5ebdfe86ac22d2414d0f61ba75e85c40769829f8a882a30d5493c168d5488d"
    );
    assert_eq!(
        ContentDigest::of_bytes(SLACK_OLD_RECIPES).as_str(),
        "d7b402aac2af7bd91b8069df075bae55a4208d16f27c8d3d497e3bdcc8d90e32"
    );
    let old = IntegrationBundle::parse(SLACK_OLD_PLUGIN, None, SLACK_OLD_RECIPES).unwrap();
    let current = IntegrationBundle::parse(SLACK_PLUGIN, None, SLACK_RECIPES).unwrap();
    let library = bundled_integration_library().unwrap();
    assert_eq!(
        old.package().identity.publisher,
        current.package().identity.publisher
    );
    assert_eq!(old.package().identity.name, current.package().identity.name);
    assert_eq!(old.package().identity.version, rev("0.1.0"));
    assert_eq!(current.package().identity.version, rev("0.2.0"));
    assert_ne!(old.digest(), current.digest());
    for bundle in [&old, &current] {
        assert!(
            library
                .get(&bundle.package().identity, bundle.digest())
                .is_some()
        );
        assert_eq!(bundle.package().known_exports.len(), 1);
        assert_eq!(
            bundle.package().known_exports[0].id,
            id("slack-mcp-setup.service")
        );
        assert_eq!(
            bundle.package().known_exports[0].role,
            ExportRole::ServiceConnector
        );
    }
    assert!(
        library
            .get(&old.package().identity, current.digest())
            .is_none()
    );
    assert!(
        library
            .get(&current.package().identity, old.digest())
            .is_none()
    );
}

#[test]
fn slack_both_explicit_strategies_refuse_activation_without_fallback() {
    let old = IntegrationBundle::parse(SLACK_OLD_PLUGIN, None, SLACK_OLD_RECIPES).unwrap();
    let bundle = IntegrationBundle::parse(SLACK_PLUGIN, None, SLACK_RECIPES).unwrap();
    assert_eq!(
        bundle
            .package()
            .strategies
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [id("provider-mcp-setup"), id("toolkit-rest-guidance")]
    );
    assert_eq!(bundle.package().strategies[0].kind, StrategyKind::RemoteMcp);
    assert_eq!(
        bundle.package().strategies[1].kind,
        StrategyKind::ConfigurationRecipe
    );
    assert!(bundle.servers().is_empty());
    let remote = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    for selected_bundle in [&old, &bundle] {
        for recipe in &selected_bundle.recipes().recipes {
            assert!(
                matches!(&recipe.plan, RecipePlan::GuidanceOnly { blockers } if !blockers.is_empty())
            );
            assert!(
                recipe
                    .prerequisites
                    .iter()
                    .any(|p| p.kind == PrerequisiteKind::DeveloperRegistration)
            );
            let mut manifest = selection(&remote);
            manifest.package = selected_bundle.package().identity.clone();
            manifest.package_content_digest = selected_bundle.package().content_digest().unwrap();
            manifest.strategy = recipe.strategy.clone();
            assert!(matches!(
                selected_bundle.validate_selection(
                    selected_bundle.digest(),
                    manifest.clone(),
                    &host()
                ),
                Err(LibrarySelectionError::GuidanceOnly)
            ));
            assert!(matches!(
                selected_bundle.validate_selection(&digest("stale"), manifest, &host()),
                Err(LibrarySelectionError::StaleBundle)
            ));
        }
    }
    for recipe in &bundle.recipes().recipes {
        for kind in [
            PrerequisiteKind::ClientAuthentication,
            PrerequisiteKind::ApplicationBinding,
        ] {
            assert!(recipe.prerequisites.iter().any(|p| p.kind == kind));
        }
    }
    assert!(
        bundle.recipes().recipes[1]
            .prerequisites
            .iter()
            .any(|p| p.kind == PrerequisiteKind::ArtifactPreparation)
    );
    let mut absent = selection(&remote);
    absent.package = bundle.package().identity.clone();
    absent.package_content_digest = bundle.package().content_digest().unwrap();
    absent.strategy = id("setup");
    assert!(matches!(
        bundle.validate_selection(bundle.digest(), absent, &host()),
        Err(LibrarySelectionError::Invalid(_))
    ));
}

#[test]
fn slack_prerequisite_change_cannot_replace_a_retained_version() {
    let mut library = bundled_integration_library().unwrap();
    let current = IntegrationBundle::parse(SLACK_PLUGIN, None, SLACK_RECIPES).unwrap();
    let mut recipes: serde_json::Value = serde_json::from_slice(SLACK_RECIPES).unwrap();
    recipes["recipes"][1]["prerequisites"][0]["instruction"] =
        serde_json::json!("A different local artifact is now required");
    let changed =
        IntegrationBundle::parse(SLACK_PLUGIN, None, &serde_json::to_vec(&recipes).unwrap())
            .unwrap();
    assert_eq!(current.package(), changed.package());
    assert_ne!(current.digest(), changed.digest());
    assert!(matches!(
        library.insert(changed),
        Err(IntegrationLibraryError::ImmutableConflict)
    ));
    assert!(
        library
            .get(&current.package().identity, current.digest())
            .is_some()
    );
    let old = IntegrationBundle::parse(SLACK_OLD_PLUGIN, None, SLACK_OLD_RECIPES).unwrap();
    assert!(library.get(&old.package().identity, old.digest()).is_some());
    assert_eq!(library.entries().count(), 14);
}

const CALENDAR_OLD_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/google-calendar-api-setup/0.1.0/plugin.json");
const CALENDAR_OLD_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/google-calendar-api-setup/0.1.0/recipes.json");
const CALENDAR_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/google-calendar-api-setup/0.2.0/plugin.json");
const CALENDAR_MCP: &[u8] =
    include_bytes!("../integrations/connectors/google-calendar-api-setup/0.2.0/mcp.json");
const CALENDAR_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/google-calendar-api-setup/0.2.0/recipes.json");

#[test]
fn calendar_versions_remain_distinct_and_historical_selection_does_not_upgrade() {
    assert_eq!(
        ContentDigest::of_bytes(CALENDAR_OLD_PLUGIN).as_str(),
        "6e36ded4e51af6c7965e41f9941c64f9e4ecd0234a1e4fb5da4dd76bcd0cf6fb"
    );
    assert_eq!(
        ContentDigest::of_bytes(CALENDAR_OLD_RECIPES).as_str(),
        "7fc9c3bfaddc87eedf82bc396a42abf245f83b26c0cabcf5a3dabaa18cb187b8"
    );
    let old = IntegrationBundle::parse(CALENDAR_OLD_PLUGIN, None, CALENDAR_OLD_RECIPES).unwrap();
    let current =
        IntegrationBundle::parse(CALENDAR_PLUGIN, Some(CALENDAR_MCP), CALENDAR_RECIPES).unwrap();
    let library = bundled_integration_library().unwrap();
    assert_eq!(
        old.package().identity.publisher,
        current.package().identity.publisher
    );
    assert_eq!(old.package().identity.name, current.package().identity.name);
    assert_eq!(old.package().identity.version, rev("0.1.0"));
    assert_eq!(current.package().identity.version, rev("0.2.0"));
    assert_ne!(old.digest(), current.digest());
    for bundle in [&old, &current] {
        assert_eq!(bundle.package().known_exports.len(), 1);
        assert!(
            library
                .get(&bundle.package().identity, bundle.digest())
                .is_some()
        );
        assert_eq!(
            bundle.package().known_exports[0].id,
            id("google-calendar-api-setup.service")
        );
        assert_eq!(
            bundle.package().known_exports[0].role,
            ExportRole::ServiceConnector
        );
    }
    assert!(
        library
            .get(&old.package().identity, current.digest())
            .is_none()
    );
    assert!(
        library
            .get(&current.package().identity, old.digest())
            .is_none()
    );
    let mut historical = selection(&current);
    historical.package = old.package().identity.clone();
    historical.package_content_digest = old.package().content_digest().unwrap();
    historical.strategy = id("setup");
    assert!(matches!(
        old.validate_selection(old.digest(), historical, &host()),
        Err(LibrarySelectionError::GuidanceOnly)
    ));
}

#[test]
fn calendar_hosted_selection_and_rest_guidance_are_explicit_and_never_substituted() {
    let bundle =
        IntegrationBundle::parse(CALENDAR_PLUGIN, Some(CALENDAR_MCP), CALENDAR_RECIPES).unwrap();
    assert_eq!(
        bundle
            .package()
            .strategies
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [id("hosted-mcp-preview"), id("toolkit-rest")]
    );
    assert_eq!(bundle.package().strategies[0].kind, StrategyKind::RemoteMcp);
    assert_eq!(
        bundle.package().strategies[1].kind,
        StrategyKind::ConfigurationRecipe
    );
    assert_eq!(bundle.recipes().recipes.len(), 2);
    assert_eq!(bundle.servers().len(), 1);
    let PortableMcpServer::StreamableHttp { url, headers } =
        &bundle.servers()["calendar-hosted-preview"]
    else {
        panic!("Calendar preview must retain the documented transport")
    };
    assert_eq!(url.as_str(), "https://calendarmcp.googleapis.com/mcp/v1");
    assert!(headers.is_empty());
    assert!(matches!(
        &bundle.recipes().recipes[0].plan,
        RecipePlan::RemoteMcp {
            read_only: false,
            read_only_evidence: None,
            ..
        }
    ));
    let selected = selection(&bundle);
    assert!(
        bundle
            .validate_selection(bundle.digest(), selected.clone(), &host())
            .is_ok()
    );
    for mutation in ["endpoint", "publisher", "auth", "host"] {
        let mut selected = selected.clone();
        let mut capabilities = host();
        match mutation {
            "endpoint" => {
                if let ComponentSource::RemoteService { endpoint, .. } =
                    &mut selected.components[0].source
                {
                    *endpoint = "https://www.googleapis.com/".parse().unwrap();
                }
            }
            "publisher" => {
                if let ComponentSource::RemoteService { publisher, .. } =
                    &mut selected.components[0].source
                {
                    *publisher = id("other-provider");
                }
            }
            "auth" => {
                if let Export::ServiceConnector(export) = &mut selected.exports[0] {
                    export.authentication = AuthenticationRequirement::None;
                }
            }
            "host" => capabilities.runtimes.clear(),
            _ => unreachable!(),
        }
        assert!(
            bundle
                .validate_selection(bundle.digest(), selected, &capabilities)
                .is_err()
        );
    }
    let mut rest = selected.clone();
    rest.strategy = id("toolkit-rest");
    assert!(matches!(
        bundle.validate_selection(bundle.digest(), rest, &host()),
        Err(LibrarySelectionError::GuidanceOnly)
    ));
    for unknown in ["setup", "choose-for-me"] {
        let mut absent = selected.clone();
        absent.strategy = id(unknown);
        assert!(matches!(
            bundle.validate_selection(bundle.digest(), absent, &host()),
            Err(LibrarySelectionError::Invalid(_))
        ));
    }
    for recipe in &bundle.recipes().recipes {
        assert_eq!(recipe.export, id("google-calendar-api-setup.service"));
        assert_eq!(recipe.role, ExportRole::ServiceConnector);
        for required in [
            PrerequisiteKind::Account,
            PrerequisiteKind::DeveloperRegistration,
            PrerequisiteKind::ClientAuthentication,
            PrerequisiteKind::Administrator,
            PrerequisiteKind::ApplicationBinding,
        ] {
            assert!(recipe.prerequisites.iter().any(|p| p.kind == required));
        }
    }
    assert!(
        bundle.recipes().recipes[0]
            .prerequisites
            .iter()
            .any(|p| p.kind == PrerequisiteKind::HostDeployment)
    );
    assert!(
        bundle.recipes().recipes[1]
            .prerequisites
            .iter()
            .any(|p| p.kind == PrerequisiteKind::ArtifactPreparation)
    );
}

#[test]
fn calendar_changed_current_prerequisites_cannot_replace_either_retained_version() {
    let mut library = bundled_integration_library().unwrap();
    let current =
        IntegrationBundle::parse(CALENDAR_PLUGIN, Some(CALENDAR_MCP), CALENDAR_RECIPES).unwrap();
    let mut recipes: serde_json::Value = serde_json::from_slice(CALENDAR_RECIPES).unwrap();
    recipes["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("Changed preview eligibility");
    let changed = IntegrationBundle::parse(
        CALENDAR_PLUGIN,
        Some(CALENDAR_MCP),
        &serde_json::to_vec(&recipes).unwrap(),
    )
    .unwrap();
    assert_eq!(current.package(), changed.package());
    assert_ne!(current.digest(), changed.digest());
    assert!(matches!(
        library.insert(changed),
        Err(IntegrationLibraryError::ImmutableConflict)
    ));
    assert!(
        library
            .get(&current.package().identity, current.digest())
            .is_some()
    );
    let old = IntegrationBundle::parse(CALENDAR_OLD_PLUGIN, None, CALENDAR_OLD_RECIPES).unwrap();
    assert!(library.get(&old.package().identity, old.digest()).is_some());
    assert_eq!(library.entries().count(), 14);
}

#[test]
fn remote_declaration_preserves_portability_without_host_snapshots_or_legacy_digest_changes() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let mut selected = selection(&bundle);
    selected.portable_mcp_entries[0].operation_tools = Some([(id("fixture-read"), "provider.read".into())].into());
    let legacy_digest = ContentDigest::of_bytes(&canonical_json(&selected).unwrap());
    let declaration = remote_declaration(&selected);
    let raw = serde_json::to_value(&declaration).unwrap();
    assert!(raw.get("configuration").is_none());
    assert!(raw.get("policy").is_none());
    assert!(raw["components"][0]["source"].get("configuration_revision").is_none());
    assert!(serde_json::from_value::<ActivationManifest>(raw.clone()).is_err());
    let pin = ContentDigest::of_bytes(&canonical_json(&declaration).unwrap());
    let (legacy, legacy_service) = bundle.export_portable_with_selection(bundle.digest(), selected.clone()).unwrap();
    let (portable, service) = bundle.export_portable_declaration(bundle.digest(), declaration).unwrap();
    assert_eq!(portable, legacy, "independent output bytes must not depend on host snapshots");
    let legacy_service = serde_json::to_value(legacy_service.unwrap()).unwrap();
    let service = serde_json::to_value(service.unwrap()).unwrap();
    assert_eq!(legacy_service["manifest_digest"], serde_json::json!(legacy_digest));
    assert!(legacy_service.get("selection_kind").is_none());
    assert!(legacy_service.get("declaration_digest").is_none());
    assert_eq!(service["selection_kind"], REMOTE_EXPORT_DECLARATION);
    assert_eq!(service["declaration_digest"], serde_json::json!(pin));
    assert!(service.get("manifest_digest").is_none());
    assert_ne!(pin, legacy_digest);
    let validated = selected.validate(bundle.package()).unwrap();
    assert_eq!(validated.digest(), &legacy_digest);
    assert!(!validated.check_host(&HostCapabilities { contracts:BTreeMap::new(), services:BTreeMap::new(),
        runtimes:BTreeSet::new(), placements:BTreeSet::new() }).is_compatible());
    for field in ["configuration", "policy", "source-revision", "unknown-kind"] {
        let mut changed = raw.clone();
        match field {
            "configuration" | "policy" => changed[field] = serde_json::json!({}),
            "source-revision" => changed["components"][0]["source"]["configuration_revision"] = serde_json::json!("1"),
            "unknown-kind" => changed["selection_kind"] = serde_json::json!("activation"),
            _ => unreachable!(),
        }
        match serde_json::from_value::<RemoteExportDeclaration>(changed) {
            Err(_) => {},
            Ok(value) => assert!(bundle.export_portable_declaration(bundle.digest(), value).is_err(), "{field}"),
        }
    }
}

#[test]
fn remote_declaration_refuses_changed_recipe_and_missing_schema_declarations() {
    let bundle = IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap();
    let mut selected = selection(&bundle);
    selected.portable_mcp_entries[0].operation_tools = Some([(id("fixture-read"), "provider.read".into())].into());
    let original = serde_json::to_value(remote_declaration(&selected)).unwrap();
    for mode in ["endpoint", "publisher", "auth", "write", "empty-map", "missing-contract", "missing-schema"] {
        let mut value = original.clone();
        match mode {
            "endpoint" => value["components"][0]["source"]["endpoint"] = serde_json::json!("https://different.invalid/mcp"),
            "publisher" => value["components"][0]["source"]["publisher"] = serde_json::json!("different"),
            "auth" => value["exports"][0]["authentication"] = serde_json::json!({"kind":"none"}),
            "write" => value["exports"][0]["operations"][0]["declared_effect"] = serde_json::json!("write"),
            "empty-map" => value["portable_mcp_entries"][0]["operation_tools"] = serde_json::json!({}),
            "missing-contract" => { value["components"][0]["source"].as_object_mut().unwrap().remove("observed_contract"); },
            "missing-schema" => { value["exports"][0]["operations"][0].as_object_mut().unwrap().remove("schema_digest"); },
            _ => unreachable!(),
        }
        match serde_json::from_value::<RemoteExportDeclaration>(value) {
            Err(_) => {},
            Ok(value) => assert!(bundle.export_portable_declaration(bundle.digest(), value).is_err(), "{mode}"),
        }
    }
    // Malformed selected fields must not displace pre-existing stale/guidance errors.
    let mut invalid = selected.clone(); invalid.components.clear(); invalid.exports.clear();
    assert!(matches!(bundle.export_portable(&digest("stale"), invalid.clone()),
        Err(LibraryPortableExportError::Selection(LibrarySelectionError::StaleBundle))));

}
