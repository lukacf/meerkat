use std::collections::BTreeMap;
use toolkit_package::*;
use url::Url;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn digest(value: &str) -> ContentDigest {
    ContentDigest::of_bytes(value.as_bytes())
}
fn contract(value: &str) -> ContractRequirement {
    ContractRequirement {
        name: id(value),
        version: VersionRequirement {
            major: 1,
            minimum_minor: 0,
        },
    }
}
fn package() -> IntegrationPackage {
    IntegrationPackage {
        schema_version: 1,
        identity: PackageIdentity {
            publisher: id("test"),
            name: id("connector"),
            version: rev("1.0.0"),
        },
        display_name: "Test connector".into(),
        description: "A partial discovery recipe".into(),
        provenance: Provenance {
            source: Url::parse("https://example.org/source").unwrap(),
            source_revision: rev("commit1"),
            license: "MIT".into(),
            notices: vec![],
            local_patches: vec![],
        },
        strategies: vec![SetupStrategy {
            id: id("research"),
            kind: StrategyKind::ResearchSkill,
            instructions: "Investigate the user's request and provider contracts.".into(),
            sources: vec![],
            known_requirements: vec![],
            unresolved_details: vec!["Authentication endpoint is unknown".into()],
        }],
        known_exports: vec![],
        assessments: vec![],
    }
}
fn remote_component() -> Component {
    Component {
        id: id("remote"),
        version: rev("contract1"),
        source: ComponentSource::RemoteService {
            endpoint: Url::parse("https://example.org/mcp").unwrap(),
            publisher: id("test"),
            configuration_revision: rev("1"),
            observed_contract: digest("tools schema"),
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
            owner: id("mcp-owner"),
            checks: [id("protocol-ready")].into(),
        },
        shutdown: ShutdownContract {
            owner: id("mcp-owner"),
            operation: id("disconnect"),
            maximum_wait_ms: 1000,
            timeout: ShutdownTimeout::PreserveUncertainty,
        },
        rollback: RollbackContract::OwnerReconciliation {
            owner: id("mcp-owner"),
            operation: id("reconcile"),
        },
    }
}
fn connector() -> Export {
    Export::ServiceConnector(ServiceConnectorExport {
        id: id("history"),
        components: [id("remote")].into(),
        authentication: AuthenticationRequirement::Binding {
            owner: id("auth"),
            requirement: id("workspace"),
            credential_audience: id("provider"),
        },
        operations: vec![OperationDeclaration {
            id: id("search"),
            schema_digest: digest("search"),
            declared_effect: DeclaredEffect::Read,
            required_authority: [id("history.read")].into(),
            resources: ResourceContract::Selected {
                selector: id("channel"),
                enforcement_owner: id("policy"),
            },
            result_limits: ResultLimits {
                maximum_bytes: 100000,
                maximum_items: 100,
            },
        }],
        events: vec![],
        enforcement_contract: contract("toolkit/service-policy"),
        diagnostics: id("check"),
        cleanup: id("release-binding"),
    })
}
fn frontend() -> Export {
    Export::AgentFrontend(AgentFrontendExport {
        id: id("bot"),
        components: [id("remote")].into(),
        modalities: [Modality::Text].into(),
        admission_contract: contract("toolkit/admission"),
        conversation_contract: contract("toolkit/conversation"),
        delivery_contract: contract("toolkit/delivery"),
        control_contract: None,
        live_contract: None,
        authentication: AuthenticationRequirement::None,
        diagnostics: id("check-bot"),
        cleanup: id("pause-bot"),
    })
}
fn manifest() -> ActivationManifest {
    let package = package();
    ActivationManifest {
        schema_version: 1,
        package: package.identity.clone(),
        package_content_digest: package.content_digest().unwrap(),
        strategy: id("research"),
        configuration: ConfigurationSelection {
            owner: id("config"),
            reference: id("selected"),
            revision: rev("1"),
            content_digest: digest("config"),
        },
        policy: PolicySelection {
            owner: id("policy"),
            revision: rev("1"),
            content_digest: digest("policy"),
        },
        components: vec![remote_component()],
        exports: vec![connector()],
        required_contracts: vec![],
        host_services: vec![],
        portable_mcp_entries: vec![],
    }
}
fn host() -> HostCapabilities {
    HostCapabilities {
        contracts: [
            "toolkit/service-policy",
            "toolkit/admission",
            "toolkit/conversation",
            "toolkit/delivery",
        ]
        .into_iter()
        .map(|name| (id(name), ContractVersion { major: 1, minor: 0 }))
        .collect(),
        services: BTreeMap::new(),
        runtimes: [RuntimeKind::Remote, RuntimeKind::Native].into(),
        placements: [id("remote"), id("local")].into(),
    }
}
fn service(optional: bool) -> HostServiceRequirement {
    HostServiceRequirement {
        name: id("robot/lease"),
        version: VersionRequirement {
            major: 2,
            minimum_minor: 1,
        },
        authority: [id("observe")].into(),
        availability: if optional {
            RequirementAvailability::Optional {
                degraded_behavior: "No device telemetry".into(),
            }
        } else {
            RequirementAvailability::Required
        },
    }
}

#[test]
fn guidance_only_recipe_is_valid_without_executable_claims() {
    let package = package();
    package.validate().unwrap();
    assert!(package.known_exports.is_empty());
    assert!(!package.strategies[0].unresolved_details.is_empty());
    let encoded = serde_json::to_vec(&package).unwrap();
    let decoded: IntegrationPackage = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, package);
    let mut selection = manifest();
    selection.exports.clear();
    assert!(selection.validate(&package).is_err());
}

#[test]
fn exact_package_bytes_and_strategy_are_bound() {
    let mut selection = manifest();
    let mut different_package = package();
    different_package.strategies[0]
        .instructions
        .push_str(" Altered instructions.");
    assert!(selection.clone().validate(&different_package).is_err());
    selection.strategy = id("undeclared");
    assert!(selection.validate(&package()).is_err());
}

#[test]
fn role_selection_is_independent_and_frontend_does_not_require_tools() {
    let service = manifest().validate(&package()).unwrap();
    assert_eq!(
        service.manifest().exports[0].role(),
        ExportRole::ServiceConnector
    );
    let mut selected = manifest();
    selected.exports = vec![frontend()];
    let frontend = selected.clone().validate(&package()).unwrap();
    assert!(frontend.check_host(&host()).is_compatible());
    assert_eq!(
        frontend.manifest().exports[0].role(),
        ExportRole::AgentFrontend
    );
    selected.exports.push(connector());
    assert!(selected.validate(&package()).is_ok());
}

#[test]
fn role_ids_cannot_collide_and_unselected_components_cannot_activate() {
    let mut selected = manifest();
    let Export::AgentFrontend(mut bot) = frontend() else {
        unreachable!()
    };
    bot.id = id("history");
    selected.exports.push(Export::AgentFrontend(bot));
    assert!(selected.validate(&package()).is_err());
    let mut selected = manifest();
    let mut hidden = remote_component();
    hidden.id = id("hidden");
    selected.components.push(hidden);
    assert!(selected.validate(&package()).is_err());
}

#[test]
fn graph_enforces_exact_closure_version_and_order() {
    let mut selected = manifest();
    let mut child = remote_component();
    child.id = id("daemon");
    selected.components[0]
        .dependencies
        .push(ComponentDependency {
            component: id("daemon"),
            selected_version: rev("contract1"),
        });
    assert!(selected.clone().validate(&package()).is_err());
    child.version = rev("wrong");
    selected.components.push(child.clone());
    assert!(selected.clone().validate(&package()).is_err());
    selected.components[1].version = rev("contract1");
    let result = selected.clone().validate(&package()).unwrap();
    assert_eq!(result.initialization_order(), &[id("daemon"), id("remote")]);
    assert_eq!(
        result.shutdown_order().cloned().collect::<Vec<_>>(),
        vec![id("remote"), id("daemon")]
    );
    selected.components[1]
        .dependencies
        .push(ComponentDependency {
            component: id("remote"),
            selected_version: rev("contract1"),
        });
    assert!(
        selected
            .validate(&package())
            .unwrap_err()
            .message
            .contains("cycle")
    );
}

#[test]
fn remote_contract_does_not_masquerade_as_content_pinned_process() {
    let mut selected = manifest();
    selected.components[0].runtime = RuntimeKind::Native;
    assert!(selected.validate(&package()).is_err());
    let mut json = serde_json::to_value(remote_component()).unwrap();
    json["source"]["content_digest"] = serde_json::json!(digest("fabricated"));
    assert!(serde_json::from_value::<Component>(json).is_err());
}

#[test]
fn canonical_digest_binds_schema_configuration_policy_and_role() {
    let original = manifest().validate(&package()).unwrap();
    for variant in 0..4 {
        let mut changed = manifest();
        match variant {
            0 => changed.configuration.revision = rev("2"),
            1 => changed.policy.content_digest = digest("new-policy"),
            2 => changed.exports = vec![frontend()],
            _ => {
                if let ComponentSource::RemoteService {
                    observed_contract, ..
                } = &mut changed.components[0].source
                {
                    *observed_contract = digest("new-schema");
                }
            }
        }
        assert_ne!(
            original.digest(),
            changed.validate(&package()).unwrap().digest()
        );
    }
    assert_eq!(
        ContentDigest::of_bytes(&original.canonical_bytes().unwrap()),
        *original.digest()
    );
}

#[test]
fn missing_or_incompatible_required_service_blocks_before_code_import() {
    let mut selected = manifest();
    selected.components[0].host_services.push(service(false));
    let selected = selected.validate(&package()).unwrap();
    let mut capabilities = host();
    assert!(!selected.check_host(&capabilities).is_compatible());
    capabilities.services.insert(
        id("robot/lease"),
        HostServiceOffer {
            version: ContractVersion { major: 2, minor: 0 },
            authority: [id("observe")].into(),
        },
    );
    assert!(!selected.check_host(&capabilities).is_compatible());
    capabilities
        .services
        .get_mut(&id("robot/lease"))
        .unwrap()
        .version
        .minor = 1;
    assert!(selected.check_host(&capabilities).is_compatible());
    capabilities
        .services
        .get_mut(&id("robot/lease"))
        .unwrap()
        .authority
        .clear();
    assert!(!selected.check_host(&capabilities).is_compatible());
}

#[test]
fn optional_service_has_explicit_degraded_outcome() {
    let mut selected = manifest();
    selected.components[0].host_services.push(service(true));
    let result = selected
        .clone()
        .validate(&package())
        .unwrap()
        .check_host(&host());
    assert!(result.is_compatible());
    assert_eq!(result.degraded[0].behavior, "No device telemetry");
    selected.components[0].host_services[0].availability = RequirementAvailability::Optional {
        degraded_behavior: " ".into(),
    };
    assert!(selected.validate(&package()).is_err());
}

#[test]
fn generic_projection_rejects_toolkit_dependent_components() {
    let mut selected = manifest();
    selected.portable_mcp_entries.push(PortableMcpEntry {
        environment_secret_slots: None,
        operation_tools: None,
        name: id("mcp"),
        component: id("remote"),
    });
    assert!(selected.clone().validate(&package()).is_ok());
    selected.components[0].activation_scope = ActivationScope::ToolkitRequired {
        activation_contract: contract("toolkit/activation"),
    };
    assert!(selected.validate(&package()).is_err());
    let mut selected = manifest();
    selected.portable_mcp_entries.push(PortableMcpEntry {
        environment_secret_slots: None,
        operation_tools: None,
        name: id("mcp"),
        component: id("remote"),
    });
    selected.components[0].host_services.push(service(false));
    assert!(selected.validate(&package()).is_err());
}

fn event(delivery: EventDelivery) -> EventDeclaration {
    EventDeclaration {
        id: id("changed"),
        payload_schema: digest("payload"),
        authenticated_source_contract: contract("toolkit/event-source"),
        consumer_contract: contract("toolkit/event-consumer"),
        semantics: EventSemantics {
            identity: id("event-id"),
            ordering: EventOrdering::Unordered,
            maximum_age_ms: 1000,
            maximum_in_flight: 1,
            backpressure: Backpressure::PauseProducer,
            delivery,
        },
    }
}
fn durable() -> EventDelivery {
    EventDelivery::Durable {
        producer_handoff: id("producer"),
        consumer_acknowledgment: id("consumer"),
        cursor: id("cursor"),
        deduplication: id("dedup"),
        retention_ms: 2000,
        recovery: id("recover"),
    }
}
#[test]
fn durability_is_explicit_and_cannot_be_combined_with_declared_loss() {
    let mut selected = manifest();
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.events.push(event(durable()));
    assert!(selected.clone().validate(&package()).is_ok());
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.events[0].semantics.backpressure = Backpressure::DeclaredLoss;
    assert!(selected.clone().validate(&package()).is_err());
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.events[0].semantics.delivery = EventDelivery::BestEffort {
        loss: LossBehavior::CoalesceLatest { key: id("sensor") },
        recovery: BestEffortRecovery::NextEvent,
    };
    assert!(selected.validate(&package()).is_ok());
}

#[test]
fn durable_contract_cannot_omit_consumer_acknowledgment_or_underretain() {
    let mut value = serde_json::to_value(durable()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("consumer_acknowledgment");
    assert!(serde_json::from_value::<EventDelivery>(value).is_err());
    let mut selected = manifest();
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    let mut declaration = event(durable());
    declaration.semantics.maximum_age_ms = 3000;
    export.events.push(declaration);
    assert!(selected.validate(&package()).is_err());
}

#[test]
fn realtime_frontend_requires_live_owner_not_just_process_readiness() {
    let mut selected = manifest();
    let Export::AgentFrontend(mut bot) = frontend() else {
        unreachable!()
    };
    bot.modalities.insert(Modality::RealtimeVoice);
    selected.exports = vec![Export::AgentFrontend(bot.clone())];
    assert!(selected.clone().validate(&package()).is_err());
    bot.live_contract = Some(contract("toolkit/live"));
    selected.exports = vec![Export::AgentFrontend(bot)];
    let selected = selected.validate(&package()).unwrap();
    assert!(!selected.check_host(&host()).is_compatible());
}

#[test]
fn unclassified_effect_declaration_remains_data_and_does_not_grant_execution() {
    let mut selected = manifest();
    let Export::ServiceConnector(export) = &mut selected.exports[0] else {
        unreachable!()
    };
    export.operations[0].declared_effect = DeclaredEffect::Unknown;
    let selected = selected.validate(&package()).unwrap();
    let Export::ServiceConnector(export) = &selected.manifest().exports[0] else {
        unreachable!()
    };
    assert_eq!(
        export.operations[0].declared_effect,
        DeclaredEffect::Unknown
    );
    // The package API deliberately returns neither grants nor binding receipts.
    assert!(selected.check_host(&host()).is_compatible());
}

#[test]
fn content_pins_ids_and_urls_reject_malformed_or_embedded_secrets() {
    assert!(ContentDigest::parse("latest").is_err());
    assert!(Id::new("../../plugin").is_err());
    assert!(serde_json::from_str::<Id>("\"has spaces\"").is_err());
    for endpoint in [
        "http://example.org/mcp",
        "https://user:password@example.org/mcp",
        "https://example.org/mcp#token",
    ] {
        let mut selected = manifest();
        if let ComponentSource::RemoteService { endpoint: url, .. } =
            &mut selected.components[0].source
        {
            *url = Url::parse(endpoint).unwrap();
        }
        assert!(selected.validate(&package()).is_err(), "{endpoint}");
    }
    let mut selected = manifest();
    if let ComponentSource::RemoteService { endpoint, .. } = &mut selected.components[0].source {
        *endpoint = Url::parse("http://127.0.0.1:7777/mcp").unwrap();
    }
    assert!(selected.validate(&package()).is_ok());
}

#[test]
fn local_entrypoint_cannot_escape_verified_artifact() {
    let mut selected = manifest();
    let component = &mut selected.components[0];
    component.runtime = RuntimeKind::Native;
    component.lifetime = ComponentLifetime::Deployment;
    component.source = ComponentSource::Artifact {
        location: Url::parse("https://example.org/package.tar").unwrap(),
        content_digest: digest("artifact"),
        dependencies: ArtifactDependencies::SelfContained,
        entrypoint: Entrypoint {
            executable: "../bin/mcp".into(),
            arguments: vec![],
            python_console_script: None,
        },
    };
    assert!(selected.clone().validate(&package()).is_err());
    if let ComponentSource::Artifact { entrypoint, .. } = &mut selected.components[0].source {
        entrypoint.executable = "bin/mcp".into();
    }
    assert!(selected.validate(&package()).is_ok());
}

#[test]
fn state_updates_require_exact_prior_format_and_preserve_removed_state() {
    let mut selected = manifest();
    selected.components[0].state.push(StateContract {
        namespace: id("channel/inbox"),
        owner: id("inbox"),
        format: rev("v1"),
        migrations: vec![],
    });
    let original = selected.clone().validate(&package()).unwrap();
    selected.components[0].state[0].format = rev("v2");
    let candidate = selected.clone().validate(&package()).unwrap();
    assert!(candidate.state_changes_from(&original).is_err());
    selected.components[0].state[0]
        .migrations
        .push(StateMigration {
            from: rev("v1"),
            operation: id("migrate"),
            artifact_digest: digest("migration"),
            reversible: false,
        });
    let candidate = selected.clone().validate(&package()).unwrap();
    let changes = candidate.state_changes_from(&original).unwrap();
    assert!(
        matches!(&changes[0], StateChange::Migration { from, to, .. } if *from == rev("v1") && *to == rev("v2"))
    );
    selected.components[0].state.clear();
    let candidate = selected.validate(&package()).unwrap();
    assert!(matches!(
        &candidate.state_changes_from(&original).unwrap()[0],
        StateChange::RetainedOnRemoval { .. }
    ));
}

#[test]
fn catalog_is_immutable_and_local_research_is_not_promoted() {
    let mut catalog = PackageCatalog::default();
    let package = package();
    let origin = CatalogOrigin::LocalInvestigation {
        decision: EvidenceReference {
            owner: id("setup"),
            record: id("decision1"),
            revision: rev("1"),
        },
    };
    catalog.insert(package.clone(), origin.clone()).unwrap();
    assert_eq!(catalog.entries().count(), 1);
    catalog
        .insert(
            package.clone(),
            CatalogOrigin::Curated {
                catalog: id("supported"),
                revision: rev("1"),
            },
        )
        .unwrap();
    assert_eq!(catalog.get(&package.identity).unwrap().origin, origin);
    let mut changed = package;
    changed.description.push_str(" altered");
    assert_eq!(
        catalog.insert(changed, origin).unwrap_err(),
        CatalogError::ImmutableConflict
    );
}

#[test]
fn catalog_support_claim_requires_evidence_and_cannot_grant_access() {
    let mut package = package();
    package.assessments.push(SupportAssessment {
        status: SupportStatus::Supported,
        scope: "Test environment".into(),
        source_revision: rev("commit1"),
        assessed_at_unix_ms: 1,
        evidence: vec![],
    });
    assert!(package.validate().is_err());
    package.assessments[0].evidence.push(EvidenceReference {
        owner: id("conformance"),
        record: id("test1"),
        revision: rev("1"),
    });
    assert!(package.validate().is_ok());
}

#[test]
fn missing_required_fields_and_unknown_contract_fields_are_rejected() {
    let mut value = serde_json::to_value(manifest()).unwrap();
    value.as_object_mut().unwrap().remove("policy");
    assert!(serde_json::from_value::<ActivationManifest>(value).is_err());
    let mut value = serde_json::to_value(manifest()).unwrap();
    value["assume_ready"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ActivationManifest>(value).is_err());
}

#[test]
fn known_strategy_requirements_cannot_disappear_at_activation() {
    let mut package = package();
    package.strategies[0]
        .known_requirements
        .push(service(false));
    let mut selected = manifest();
    selected.package_content_digest = package.content_digest().unwrap();
    assert!(selected.clone().validate(&package).is_err());
    selected.host_services.push(service(false));
    let selection = selected.validate(&package).unwrap();
    assert!(!selection.check_host(&host()).is_compatible());
}

#[test]
fn known_role_cannot_silently_change_in_selected_export() {
    let mut package = package();
    package.known_exports.push(ExportHint {
        id: id("history"),
        role: ExportRole::AgentFrontend,
        description: "Frontend".into(),
    });
    let mut selected = manifest();
    selected.package_content_digest = package.content_digest().unwrap();
    assert!(selected.validate(&package).is_err());
}

#[test]
fn agent_plugin_manifest_ignores_unknown_fields_and_namespaces_without_semantics() {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "$schema": AGENT_PLUGIN_SCHEMA, "name": "my-plugin", "unexpected": { "shell": "do something" },
        "extensions": { "org.unknown.client": ["not our schema"] },
        "version": "not-semver", "homepage": "not-url"
    })).unwrap();
    let imported = ImportedPluginManifest::parse(&bytes).unwrap();
    assert_eq!(imported.diagnostics.len(), 1);
    assert!(imported.toolkit_package().unwrap().is_none());
    assert_eq!(imported.manifest.version.as_deref(), Some("not-semver"));
    let invalid = serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "my-plugin", "author": { "unexpected": true } });
    assert!(ImportedPluginManifest::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn agent_plugin_wrong_schema_and_name_are_not_guessed() {
    for name in ["", "Bad", "bad--name", "bad..name", "-bad", "bad-"] {
        let value = serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": name });
        assert!(ImportedPluginManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let value =
        serde_json::json!({ "$schema": "https://attacker.invalid/schema.json", "name": "valid" });
    assert!(ImportedPluginManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let value =
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "valid", "extensions": 7 });
    let parsed = ImportedPluginManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(parsed.diagnostics.len(), 1);
}

#[test]
fn portable_mcp_entry_failures_are_isolated_without_transport_fallback() {
    let value = serde_json::json!({ "$schema": AGENT_PLUGIN_MCP_SCHEMA, "mcpServers": {
        "good": { "type": "stdio", "command": "node", "args": ["${PLUGIN_ROOT}/index.js"] },
        "bad": { "type": "stdio", "command": "node index.js" },
        "unknown": { "type": "future", "url": "https://example.org/mcp" },
        "mixed": { "type": "streamable-http", "url": "https://example.org/mcp", "command": "node" },
        "legacy": { "type": "sse", "url": "https://example.org/sse" }
    }});
    let imported = ImportedMcpServers::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(imported.servers.len(), 2);
    assert_eq!(imported.diagnostics.len(), 3);
    assert!(matches!(
        imported.servers["legacy"],
        PortableMcpServer::LegacySse { .. }
    ));
}

#[test]
fn portable_environment_and_header_constraints_cannot_be_bypassed() {
    let value = serde_json::json!({ "$schema": AGENT_PLUGIN_MCP_SCHEMA, "mcpServers": {
        "reserved": { "type": "stdio", "command": "node", "env": { "PLUGIN_ROOT": "/tmp" } },
        "escape": { "type": "stdio", "command": "./../evil" },
        "cwd": { "type": "stdio", "command": "node", "cwd": "${PLUGIN_DATA}/../../outside" },
        "headers": { "type": "streamable-http", "url": "https://example.org/mcp", "headers": { "X-Tenant": "a", "x-tenant": "b" } },
        "injection": { "type": "streamable-http", "url": "https://example.org/mcp", "headers": { "X-Tenant": "a\r\nAuthorization: value" } }
    }});
    let imported = ImportedMcpServers::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(imported.servers.is_empty());
    assert_eq!(imported.diagnostics.len(), 5);
}

#[test]
fn portable_import_rejects_duplicate_transport_and_credential_declarations() {
    let entries = [
        r#"{"type":"sse","type":"streamable-http","url":"https://example.org/mcp"}"#,
        r#"{"type":"streamable-http","url":"https://first.invalid/mcp","url":"https://second.invalid/mcp"}"#,
        r#"{"type":"streamable-http","url":"https://example.org/mcp","headers":{"Authorization":"synthetic-first","Authorization":"synthetic-second"}}"#,
        r#"{"type":"streamable-http","url":"https://example.org/mcp","headers":{"Authorization":"synthetic-first","\u0041uthorization":"synthetic-second"}}"#,
        r#"{"type":"streamable-http","url":"https://example.org/mcp","headers":{"Authorization":"synthetic-first"},"headers":{}}"#,
        r#"{"type":"stdio","command":"node","env":{"TOKEN":"synthetic-first","TOKEN":"synthetic-second"}}"#,
    ];
    for entry in entries {
        let raw = format!(
            r#"{{"$schema":"{AGENT_PLUGIN_MCP_SCHEMA}","mcpServers":{{"valid":{{"type":"stdio","command":"node"}},"ambiguous":{entry}}}}}"#
        );
        let error = ImportedMcpServers::parse(raw.as_bytes()).unwrap_err();
        assert_eq!(error.path, "mcp.json");
        assert_eq!(error.message, "invalid or duplicate-key JSON");
        assert!(!error.to_string().contains("synthetic-"));
    }
    for servers in [
        r#""mcpServers":{},"mcpServers":{"changed":{"type":"stdio","command":"node"}}"#,
        r#""mcpServers":{"same":{"type":"sse","url":"https://example.org/mcp"},"same":{"type":"streamable-http","url":"https://example.org/mcp"}}"#,
    ] {
        let raw = format!(r#"{{"$schema":"{AGENT_PLUGIN_MCP_SCHEMA}",{servers}}}"#);
        assert!(ImportedMcpServers::parse(raw.as_bytes()).is_err());
    }
}

#[test]
fn portable_import_cannot_erase_duplicate_toolkit_contract_metadata() {
    for fields in [
        r#""name":"first","name":"second""#,
        r#""name":"plain","extensions":{"ai.rkat.toolkit":{"invalid":"synthetic-secret"}},"extensions":{}"#,
        r#""name":"plain","extensions":{"ai.rkat.toolkit":{},"ai.rkat.toolkit":null}"#,
        r#""name":"plain","extensions":{"org.other.client":{"nested":[{"key":"synthetic-first","key":"synthetic-second"}]}}"#,
    ] {
        let raw = format!(r#"{{"$schema":"{AGENT_PLUGIN_SCHEMA}",{fields}}}"#);
        let error = ImportedPluginManifest::parse(raw.as_bytes()).unwrap_err();
        assert_eq!(error.path, "plugin.json");
        assert_eq!(error.message, "invalid or duplicate-key JSON");
        assert!(!error.to_string().contains("synthetic-"));
    }

    let mut package = package();
    package.strategies[0].known_requirements = vec![service(false)];
    let raw = serde_json::to_string(&serde_json::json!({
        "$schema": AGENT_PLUGIN_SCHEMA, "name": "required-service",
        "extensions": { TOOLKIT_NAMESPACE: package }
    }))
    .unwrap();
    let imported = ImportedPluginManifest::parse(raw.as_bytes()).unwrap();
    assert_eq!(imported.toolkit_package().unwrap(), Some(package.clone()));
    let requirements = serde_json::to_value(&package.strategies[0].known_requirements).unwrap();
    let declaration = format!(r#""known_requirements":{requirements}"#);
    let erased = raw.replace(
        &declaration,
        &format!(r#"{declaration},"known_requirements":[]"#),
    );
    assert_ne!(erased, raw);
    assert!(ImportedPluginManifest::parse(erased.as_bytes()).is_err());
    let narrowed = raw.replace(r#""major":2"#, r#""major":99,"major":2"#);
    assert_ne!(narrowed, raw);
    assert!(ImportedPluginManifest::parse(narrowed.as_bytes()).is_err());
}

#[test]
fn plugin_placeholders_expand_once_and_preserve_unknown_syntax() {
    let value = "${PLUGIN_ROOT}/${PLUGIN_DATA}/${HOME}/$(shell)/${PLUGIN_UNKNOWN}";
    assert_eq!(
        expand_plugin_placeholders(value, "${PLUGIN_DATA}", "/data"),
        "${PLUGIN_DATA}//data/${HOME}/$(shell)/${PLUGIN_UNKNOWN}"
    );
}

#[test]
fn plugin_root_reads_are_bounded_and_require_regular_files() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("plugin.json"),
        serde_json::to_vec(&serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "valid" }))
            .unwrap(),
    )
    .unwrap();
    std::fs::write(temp.path().join("large"), b"123456").unwrap();
    std::fs::create_dir(temp.path().join("directory")).unwrap();
    let root = PluginRoot::open(temp.path()).unwrap();
    assert_eq!(root.read_manifest().unwrap().manifest.name, "valid");
    assert!(root.read("./large", 5).is_err());
    assert!(root.read("./directory", 100).is_err());
    assert!(root.read("./../escape", 100).is_err());
}

#[cfg(unix)]
#[test]
fn plugin_root_rejects_symlink_escape_and_survives_root_path_replacement() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path().join("plugin");
    std::fs::create_dir(&root_path).unwrap();
    std::fs::write(root_path.join("inside"), b"safe").unwrap();
    std::fs::write(temp.path().join("private"), b"secret").unwrap();
    symlink("../private", root_path.join("escape")).unwrap();
    symlink("inside", root_path.join("allowed")).unwrap();
    let root = PluginRoot::open(&root_path).unwrap();
    assert!(root.read("./escape", 100).is_err());
    assert_eq!(root.read("./allowed", 100).unwrap(), b"safe");
    std::fs::rename(&root_path, temp.path().join("original")).unwrap();
    std::fs::create_dir(&root_path).unwrap();
    std::fs::write(root_path.join("inside"), b"replacement").unwrap();
    assert_eq!(root.read("./inside", 100).unwrap(), b"safe");
}

fn mcpb_document() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": "0.3", "name": "example", "version": "1.0.0", "description": "An MCP bundle",
        "author": { "name": "Publisher" },
        "server": { "type": "node", "entry_point": "server/index.js", "mcp_config": { "command": "node", "args": ["${__dirname}/server/index.js"], "env": { "API_KEY": "${user_config.api_key}" } } },
        "user_config": { "api_key": { "type": "string", "title": "API key", "sensitive": true, "required": true } },
        "license": "MIT"
    })
}

#[test]
fn mcpb_import_preserves_guidance_and_secret_requirements_without_expansion() {
    let bytes = serde_json::to_vec(&mcpb_document()).unwrap();
    let imported = ImportedMcpbManifest::parse(&bytes).unwrap();
    assert_eq!(imported.source_digest(), &ContentDigest::of_bytes(&bytes));
    assert_eq!(
        imported
            .sensitive_fields()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        vec!["api_key"]
    );
    assert_eq!(
        imported.manifest().server.mcp_config.env["API_KEY"],
        "${user_config.api_key}"
    );
    assert!(
        imported
            .unresolved_contracts()
            .contains(&McpbUnresolvedContract::BundleArtifactAndDependencyPins)
    );
    assert!(
        imported
            .unresolved_contracts()
            .contains(&McpbUnresolvedContract::HostOwnedConfigurationAndSecretBindings)
    );
}

#[test]
fn mcpb_import_rejects_bundle_escape_secrets_and_unknown_runtime_version() {
    let mut value = mcpb_document();
    value["server"]["entry_point"] = serde_json::json!("../outside");
    assert!(ImportedMcpbManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = mcpb_document();
    value["user_config"]["api_key"]["default"] = serde_json::json!("embedded-secret");
    assert!(ImportedMcpbManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = mcpb_document();
    value["manifest_version"] = serde_json::json!("0.4");
    assert!(ImportedMcpbManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

fn mcpb_with_raw_member(name: &str, raw: &str) -> Vec<u8> {
    let mut document = mcpb_document();
    document.as_object_mut().unwrap().remove(name);
    let mut bytes = serde_json::to_vec(&document).unwrap();
    assert_eq!(bytes.pop(), Some(b'}'));
    bytes.extend_from_slice(format!(",{name:?}:{raw}}}").as_bytes());
    bytes
}

fn mcpb_duplicate_configurations() -> [Vec<u8>; 2] {
    [
        r#"{"api_key":{"type":"string","title":"Private declaration","sensitive":true,"required":true},"api_key":{"type":"string","title":"Replacement","default":"PRIVATE-FIXTURE"}}"#,
        r#"{"api_key":{"type":"string","title":"Private declaration","sensitive":true,"required":true},"api\u005fkey":{"type":"string","title":"Replacement","default":"PRIVATE-FIXTURE"}}"#,
    ]
    .map(|raw| mcpb_with_raw_member("user_config", raw))
}

#[test]
fn mcpb_canonical_json_rejects_duplicate_sensitive_configuration() {
    for bytes in mcpb_duplicate_configurations() {
        let error = ImportedMcpbManifest::parse(&bytes)
            .err()
            .expect("duplicate keys must not erase the original sensitive declaration");
        assert_eq!(error.path, "manifest.json");
        assert_eq!(error.message, "invalid MCPB manifest");
    }
}

#[test]
fn mcpb_canonical_json_rejects_nested_duplicate_declarations() {
    for (name, raw) in [
        (
            "compatibility",
            r#"{"platforms":{"linux":true,"linux":false}}"#,
        ),
        (
            "vendor",
            r#"{"rules":[{"sensitive":true,"sensitive":false}]}"#,
        ),
        (
            "server",
            r#"{"type":"node","entry_point":"server/index.js","mcp_config":{"command":"node","env":{"TOKEN":"PRIVATE-FIXTURE","TOKEN":"replacement"}}}"#,
        ),
    ] {
        let error = ImportedMcpbManifest::parse(&mcpb_with_raw_member(name, raw))
            .err()
            .expect("nested duplicate declarations must not be normalized away");
        assert_eq!(error.path, "manifest.json");
        assert_eq!(error.message, "invalid MCPB manifest");
    }
}

#[test]
fn mcpb_canonical_json_preserves_raw_identity_and_opaque_metadata() {
    let bytes = mcpb_with_raw_member("vendor", r#"{"rules":[{"scope":"one"},{"scope":"two"}]}"#);
    let imported = ImportedMcpbManifest::parse(&bytes).unwrap();
    let spaced = [b"\n ".as_slice(), bytes.as_slice(), b" \n".as_slice()].concat();
    let same_manifest = ImportedMcpbManifest::parse(&spaced).unwrap();
    assert_eq!(imported.manifest(), same_manifest.manifest());
    assert_eq!(imported.source_digest(), &ContentDigest::of_bytes(&bytes));
    assert_eq!(
        same_manifest.source_digest(),
        &ContentDigest::of_bytes(&spaced)
    );
    assert_ne!(imported.source_digest(), same_manifest.source_digest());
    assert_eq!(
        imported.manifest().metadata["vendor"]["rules"][1]["scope"],
        "two"
    );
    assert_eq!(
        imported
            .sensitive_fields()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        vec!["api_key"]
    );
}

#[test]
fn mcpb_canonical_json_inspector_refuses_duplicates_without_partial_summary() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for bytes in mcpb_duplicate_configurations() {
        let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-package-inspect"))
            .arg("mcpb")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report,
            serde_json::json!({
                "schema_version": 1, "scope": "metadata_only", "status": "invalid",
                "error": {"kind": "invalid_metadata", "path": "manifest.json", "message": "invalid MCPB manifest"}
            })
        );
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("PRIVATE-FIXTURE")
        );
    }
}

fn assert_no_null(value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => panic!("portable export emitted null"),
        serde_json::Value::Array(items) => items.iter().for_each(assert_no_null),
        serde_json::Value::Object(fields) => fields.values().for_each(assert_no_null),
        _ => {}
    }
}

fn exported_value(bytes: &[u8]) -> serde_json::Value {
    let value = serde_json::from_slice(bytes).unwrap();
    assert_no_null(&value);
    value
}

fn toolkit_manifest(package: &IntegrationPackage) -> AgentPluginManifest {
    let value = serde_json::json!({
        "$schema": AGENT_PLUGIN_SCHEMA,
        "name": "connector",
        "version": "1.0.0",
        "extensions": {
            TOOLKIT_NAMESPACE: package,
            "org.other.client": { "opaque": [1, "two", null] }
        }
    });
    ImportedPluginManifest::parse(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .manifest
}

fn portable_selection() -> ValidatedManifest {
    let mut selected = manifest();
    selected.portable_mcp_entries.push(PortableMcpEntry {
        environment_secret_slots: None,
        operation_tools: None,
        name: id("mcp"),
        component: id("remote"),
    });
    selected.validate(&package()).unwrap()
}

fn remote_entry(url: &str) -> BTreeMap<String, PortableMcpServer> {
    [(
        "mcp".to_owned(),
        PortableMcpServer::StreamableHttp {
            url: Url::parse(url).unwrap(),
            headers: BTreeMap::new(),
        },
    )]
    .into()
}

fn refusal(result: Result<PortablePluginExport, PortableExportError>) -> UnsupportedPortableExport {
    match result {
        Err(PortableExportError::Unsupported { reason, .. }) => reason,
        other => panic!("expected an unsupported export refusal, got {other:?}"),
    }
}

#[test]
fn derived_serialization_of_an_imported_manifest_is_not_a_portable_export() {
    let minimal = serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "minimal" });
    let imported = ImportedPluginManifest::parse(&serde_json::to_vec(&minimal).unwrap()).unwrap();
    let derived = serde_json::to_value(&imported.manifest).unwrap();
    assert!(derived["version"].is_null());
    assert!(ImportedPluginManifest::parse(&serde_json::to_vec(&derived).unwrap()).is_err());
}

#[test]
fn portable_export_keeps_absent_fields_absent_and_round_trips_through_the_importer() {
    let documents = [
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "minimal" }),
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "partial", "author": { "name": "A. Maintainer" } }),
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "empty-author", "author": {} }),
        serde_json::json!({
            "$schema": AGENT_PLUGIN_SCHEMA, "name": "full.plugin", "version": "not-semver",
            "description": "d", "author": { "name": "n", "email": "e@example.org", "url": "https://example.org" },
            "homepage": "not-url", "repository": "https://example.org/repo", "license": "MIT",
            "keywords": ["one", "two"], "extensions": { "org.unknown.client": ["not our schema", null] }
        }),
    ];
    for document in documents {
        let imported =
            ImportedPluginManifest::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        let export = PortablePluginExport::new(&imported.manifest, &BTreeMap::new(), None).unwrap();
        let mut exported =
            serde_json::from_slice::<serde_json::Value>(export.plugin_json()).unwrap();
        // Opaque namespaces keep their own values, including nulls inside them.
        let extensions = exported.as_object_mut().unwrap().remove("extensions");
        assert_no_null(&exported);
        if let Some(extensions) = extensions {
            assert_eq!(extensions, document["extensions"]);
            exported["extensions"] = extensions;
        }
        assert_eq!(exported, document);
        assert!(export.mcp_json().is_none());
        let reimported = ImportedPluginManifest::parse(export.plugin_json()).unwrap();
        assert!(reimported.diagnostics.is_empty());
        assert_eq!(reimported.manifest, imported.manifest);
    }
    for malformed in [
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "valid", "version": null }),
        serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "valid", "author": { "email": null } }),
    ] {
        assert!(ImportedPluginManifest::parse(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
    let mut invalid = ImportedPluginManifest::parse(
        &serde_json::to_vec(
            &serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "valid" }),
        )
        .unwrap(),
    )
    .unwrap()
    .manifest;
    invalid.name = "Not--Portable".into();
    assert!(matches!(
        PortablePluginExport::new(&invalid, &BTreeMap::new(), None),
        Err(PortableExportError::Invalid(_))
    ));
}

#[test]
fn portable_export_preserves_transport_arguments_and_placeholders_and_refuses_malformed_entries() {
    let plain = ImportedPluginManifest::parse(
        &serde_json::to_vec(
            &serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "plain" }),
        )
        .unwrap(),
    )
    .unwrap()
    .manifest;
    let document = serde_json::json!({ "$schema": AGENT_PLUGIN_MCP_SCHEMA, "mcpServers": {
        // The command does not exist: exporting must not launch or resolve it.
        "local": { "type": "stdio", "command": "./bin/absent-server",
            "args": ["${PLUGIN_ROOT}/config.json", "--literal", "$(not-a-shell)", "${PLUGIN_UNKNOWN}"],
            "env": { "SERVICE_CREDENTIAL": "${SERVICE_CREDENTIAL}" }, "cwd": "${PLUGIN_DATA}/work" },
        "bare": { "type": "stdio", "command": "node" },
        "hosted": { "type": "streamable-http", "url": "https://example.org/mcp?tenant=${TENANT}",
            "headers": { "X-Tenant": "${TENANT}" } },
        "legacy": { "type": "sse", "url": "https://example.org/sse" }
    }});
    let servers = ImportedMcpServers::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(servers.diagnostics.is_empty());
    let export = PortablePluginExport::new(&plain, &servers.servers, None).unwrap();
    let exported = exported_value(export.mcp_json().unwrap());
    assert_eq!(exported, document);
    let reimported = ImportedMcpServers::parse(export.mcp_json().unwrap()).unwrap();
    assert!(reimported.diagnostics.is_empty());
    assert_eq!(reimported.servers, servers.servers);

    let malformed = [
        PortableMcpServer::Stdio {
            command: "node index.js".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        },
        PortableMcpServer::Stdio {
            command: "node".into(),
            args: vec![],
            env: [("PLUGIN_ROOT".to_owned(), "/tmp".to_owned())].into(),
            cwd: None,
        },
        PortableMcpServer::Stdio {
            command: "node".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: Some("${PLUGIN_DATA}/../../outside".into()),
        },
        PortableMcpServer::StreamableHttp {
            url: Url::parse("https://example.org/mcp").unwrap(),
            headers: [(
                "X-Tenant".to_owned(),
                "a\r\nAuthorization: value".to_owned(),
            )]
            .into(),
        },
        PortableMcpServer::LegacySse {
            url: Url::parse("https://user:pass@example.org/sse").unwrap(),
            headers: BTreeMap::new(),
        },
    ];
    for server in malformed {
        let mut entries = servers.servers.clone();
        entries.insert("bad".into(), server);
        match PortablePluginExport::new(&plain, &entries, None) {
            Err(PortableExportError::Invalid(error)) => {
                assert!(error.path.starts_with("mcp.json:bad"))
            }
            other => panic!("malformed entry was not refused: {other:?}"),
        }
    }
}

#[test]
fn generic_clients_get_only_entries_the_selection_declared_independent() {
    let package = package();
    let namespaced = toolkit_manifest(&package);
    let selection = portable_selection();
    let export = PortablePluginExport::new(
        &namespaced,
        &remote_entry("https://example.org/mcp"),
        Some(&selection),
    )
    .unwrap();
    let reimported = ImportedMcpServers::parse(export.mcp_json().unwrap()).unwrap();
    assert_eq!(reimported.servers, remote_entry("https://example.org/mcp"));
    let plugin = ImportedPluginManifest::parse(export.plugin_json()).unwrap();
    assert_eq!(plugin.manifest, namespaced);
    assert_eq!(plugin.toolkit_package().unwrap(), Some(package.clone()));

    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &remote_entry("https://example.org/mcp"),
            None
        )),
        UnsupportedPortableExport::SelectionRequired
    );
    let plain = ImportedPluginManifest::parse(
        &serde_json::to_vec(
            &serde_json::json!({ "$schema": AGENT_PLUGIN_SCHEMA, "name": "plain" }),
        )
        .unwrap(),
    )
    .unwrap()
    .manifest;
    assert_eq!(
        refusal(PortablePluginExport::new(
            &plain,
            &remote_entry("https://example.org/mcp"),
            Some(&selection)
        )),
        UnsupportedPortableExport::SelectionWithoutToolkitPackage
    );
    let mut extra = remote_entry("https://example.org/mcp");
    extra.insert(
        "bypass".into(),
        PortableMcpServer::Stdio {
            command: "node".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        },
    );
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &extra,
            Some(&selection)
        )),
        UnsupportedPortableExport::UndeclaredEntry
    );
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &BTreeMap::new(),
            Some(&selection)
        )),
        UnsupportedPortableExport::MissingDeclaredEntry
    );
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &remote_entry("https://example.org/other"),
            Some(&selection)
        )),
        UnsupportedPortableExport::ComponentMismatch
    );
    let legacy: BTreeMap<_, _> = [(
        "mcp".to_owned(),
        PortableMcpServer::LegacySse {
            url: Url::parse("https://example.org/mcp").unwrap(),
            headers: BTreeMap::new(),
        },
    )]
    .into();
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &legacy,
            Some(&selection)
        )),
        UnsupportedPortableExport::ComponentMismatch
    );
    let mut changed = package.clone();
    changed.description = "Changed package content".into();
    assert_eq!(
        refusal(PortablePluginExport::new(
            &toolkit_manifest(&changed),
            &remote_entry("https://example.org/mcp"),
            Some(&selection)
        )),
        UnsupportedPortableExport::SelectionForAnotherPackage
    );

    // An artifact entry would need its bytes realized under the plugin root.
    let mut artifact = manifest();
    artifact.components[0].runtime = RuntimeKind::Native;
    artifact.components[0].lifetime = ComponentLifetime::Deployment;
    artifact.components[0].source = ComponentSource::Artifact {
        location: Url::parse("https://example.org/package.tar").unwrap(),
        content_digest: digest("artifact"),
        dependencies: ArtifactDependencies::SelfContained,
        entrypoint: Entrypoint {
            executable: "bin/mcp".into(),
            arguments: vec![],
            python_console_script: None,
        },
    };
    artifact.portable_mcp_entries.push(PortableMcpEntry {
        environment_secret_slots: None,
        operation_tools: None,
        name: id("mcp"),
        component: id("remote"),
    });
    let artifact = artifact.validate(&package).unwrap();
    let stdio: BTreeMap<_, _> = [(
        "mcp".to_owned(),
        PortableMcpServer::Stdio {
            command: "./bin/mcp".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        },
    )]
    .into();
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &stdio,
            Some(&artifact)
        )),
        UnsupportedPortableExport::ArtifactRealization
    );

    // A toolkit-dependent component is never declared portable, so its entry
    // cannot reach a generic client through this exporter.
    let mut dependent = manifest();
    dependent.components[0].activation_scope = ActivationScope::ToolkitRequired {
        activation_contract: ContractRequirement {
            name: id("toolkit/activation"),
            version: VersionRequirement {
                major: 2,
                minimum_minor: 0,
            },
        },
    };
    let mut declared = dependent.clone();
    declared.portable_mcp_entries.push(PortableMcpEntry {
        environment_secret_slots: None,
        operation_tools: None,
        name: id("mcp"),
        component: id("remote"),
    });
    assert!(declared.validate(&package).is_err());
    let dependent = dependent.validate(&package).unwrap();
    assert_eq!(
        refusal(PortablePluginExport::new(
            &namespaced,
            &remote_entry("https://example.org/mcp"),
            Some(&dependent)
        )),
        UnsupportedPortableExport::UndeclaredEntry
    );
    // The namespace alone still exports, and the unsupported required toolkit
    // version still refuses activation after re-import.
    let namespace_only = PortablePluginExport::new(&namespaced, &BTreeMap::new(), None).unwrap();
    assert!(namespace_only.mcp_json().is_none());
    let reimported = ImportedPluginManifest::parse(namespace_only.plugin_json())
        .unwrap()
        .toolkit_package()
        .unwrap()
        .unwrap();
    assert_eq!(reimported, package);
    let mut offered = host();
    offered.contracts.insert(
        id("toolkit/activation"),
        ContractVersion { major: 1, minor: 9 },
    );
    let report = dependent.check_host(&offered);
    assert!(!report.is_compatible());
    assert!(report.required_gaps.iter().any(|gap| matches!(
        &gap.kind,
        CompatibilityGapKind::Contract { name, .. } if *name == id("toolkit/activation")
    )));
}

#[test]
fn skill_files_export_as_inert_guidance_through_anchored_reads() {
    let package = package();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("skills/setup")).unwrap();
    std::fs::write(
        source.join("plugin.json"),
        PortablePluginExport::new(&toolkit_manifest(&package), &BTreeMap::new(), None)
            .unwrap()
            .plugin_json(),
    )
    .unwrap();
    let skill = b"# Setup\n\nRun `npx example-mcp --token $TOKEN` as an MCP server.\n";
    std::fs::write(source.join("skills/setup/SKILL.md"), skill).unwrap();
    std::fs::write(temp.path().join("private"), b"outside the plugin").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("../../../private", source.join("skills/setup/leak")).unwrap();
    let root = PluginRoot::open(&source).unwrap();
    let imported = root.read_manifest().unwrap();
    let export = PortablePluginExport::new(&imported.manifest, &BTreeMap::new(), None)
        .unwrap()
        .with_file(&root, "./skills/setup/SKILL.md")
        .unwrap();
    for escape in [
        "./../private",
        "skills/setup/SKILL.md",
        "./skills/../../private",
    ] {
        assert!(export.clone().with_file(&root, escape).is_err());
    }
    #[cfg(unix)]
    assert!(
        export
            .clone()
            .with_file(&root, "./skills/setup/leak")
            .is_err()
    );
    for reserved in ["./plugin.json", "./mcp.json"] {
        assert_eq!(
            refusal(export.clone().with_file(&root, reserved)),
            UnsupportedPortableExport::ReservedFile
        );
    }
    let destination = temp.path().join("exported");
    export.write_new(&destination).unwrap();
    assert!(export.write_new(&destination).is_err());
    let reopened = PluginRoot::open(&destination).unwrap();
    let reimported = reopened.read_manifest().unwrap();
    assert!(reimported.diagnostics.is_empty());
    assert_eq!(reimported.manifest, imported.manifest);
    assert_eq!(reimported.toolkit_package().unwrap(), Some(package));
    assert_eq!(
        reopened.read("./skills/setup/SKILL.md", 1024).unwrap(),
        skill
    );
    // Prose that mentions a server is not an MCP entry or compatibility claim.
    assert!(reopened.read("./mcp.json", 1024).is_err());
    assert_eq!(
        export.files().map(|(path, _)| path).collect::<Vec<_>>(),
        ["./skills/setup/SKILL.md"]
    );
}

#[cfg(unix)]
#[test]
fn executable_export_keeps_anchored_containment_and_regular_file_checks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("actual"), b"inert executable bytes\n").unwrap();
    std::fs::set_permissions(
        source.join("actual"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("actual", source.join("inside")).unwrap();
    std::fs::write(temp.path().join("outside"), b"outside bytes\n").unwrap();
    symlink("../outside", source.join("escape")).unwrap();
    // No writer is created. The regular-file check must refuse before a read
    // could block on the FIFO.
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(source.join("fifo"))
            .status()
            .unwrap()
            .success()
    );

    let root = PluginRoot::open(&source).unwrap();
    let export =
        PortablePluginExport::new(&toolkit_manifest(&package()), &BTreeMap::new(), None).unwrap();
    assert!(export.clone().with_file(&root, "./escape").is_err());
    assert!(export.clone().with_file(&root, "./fifo").is_err());
    assert!(export.clone().with_file(&root, "./").is_err());
    let export = export.with_file(&root, "./inside").unwrap();
    let destination = temp.path().join("exported");
    export.write_new(&destination).unwrap();
    assert!(
        std::fs::symlink_metadata(destination.join("inside"))
            .unwrap()
            .is_file()
    );
    assert_eq!(
        std::fs::read(destination.join("inside")).unwrap(),
        b"inert executable bytes\n"
    );
    assert_ne!(
        std::fs::metadata(destination.join("inside"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert!(export.write_new(&destination).is_err());
}
