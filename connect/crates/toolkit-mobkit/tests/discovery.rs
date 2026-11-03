use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolError, ToolMutationClass};
use serde_json::{Value, json, value::RawValue};
use toolkit_mobkit::discovery::{DiscoveryTools, DiscoveryToolsError, LIST_RECIPES, READ_RECIPE};
use toolkit_package::{CatalogOrigin, Id, PackageCatalog, Revision, bundled_discovery_catalog};

#[tokio::test]
async fn retained_library_discovery_requires_both_exact_digests() {
    let library = toolkit_package::bundled_integration_library().unwrap();
    let tools = DiscoveryTools::from_library(&library).unwrap();
    let page = invoke(&tools, LIST_RECIPES, json!({"limit":20}))
        .await
        .unwrap();
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 14);
    let identities: std::collections::BTreeSet<_> = entries
        .iter()
        .map(|entry| {
            (
                entry["identity"]["publisher"].as_str().unwrap(),
                entry["identity"]["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(identities.len(), 11);
    let gmail_versions: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|entry| {
            entry["identity"]["publisher"] == "meerkat-toolkit"
                && entry["identity"]["name"] == "gmail-api-setup"
        })
        .map(|entry| entry["identity"]["version"].as_str().unwrap())
        .collect();
    assert_eq!(
        gmail_versions,
        std::collections::BTreeSet::from(["0.1.0", "0.2.0"])
    );
    let slack_versions: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|entry| {
            entry["identity"]["publisher"] == "meerkat-toolkit"
                && entry["identity"]["name"] == "slack-mcp-setup"
        })
        .map(|entry| entry["identity"]["version"].as_str().unwrap())
        .collect();
    assert_eq!(
        slack_versions,
        std::collections::BTreeSet::from(["0.1.0", "0.2.0"])
    );
    let calendar_entries: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry["identity"]["publisher"] == "meerkat-toolkit"
                && entry["identity"]["name"] == "google-calendar-api-setup"
        })
        .collect();
    let calendar_versions: std::collections::BTreeSet<_> = calendar_entries
        .iter()
        .map(|entry| entry["identity"]["version"].as_str().unwrap())
        .collect();
    assert_eq!(
        calendar_versions,
        std::collections::BTreeSet::from(["0.1.0", "0.2.0"])
    );
    assert_eq!(calendar_entries.len(), 2);
    assert_ne!(calendar_entries[0]["digest"], calendar_entries[1]["digest"]);
    assert_ne!(
        calendar_entries[0]["library_digest"],
        calendar_entries[1]["library_digest"]
    );
    // A historical Calendar read cannot silently use the other version's inputs.
    for (selected, other) in [
        (calendar_entries[0], calendar_entries[1]),
        (calendar_entries[1], calendar_entries[0]),
    ] {
        assert!(
            invoke(
                &tools,
                READ_RECIPE,
                json!({
                    "identity": selected["identity"],
                    "digest": selected["digest"],
                    "library_digest": other["library_digest"]
                })
            )
            .await
            .is_err()
        );
    }
    for summary in page["entries"].as_array().unwrap() {
        let request = json!({
            "identity":summary["identity"],
            "digest":summary["digest"],
            "library_digest":summary["library_digest"]
        });
        let result = invoke(&tools, READ_RECIPE, request.clone()).await.unwrap();
        let identity = serde_json::from_value(summary["identity"].clone()).unwrap();
        let digest = serde_json::from_value(summary["library_digest"].clone()).unwrap();
        let expected = library.get(&identity, &digest).unwrap();
        assert_eq!(result["library"], serde_json::to_value(expected).unwrap());
        assert_eq!(
            result["package"],
            serde_json::to_value(expected.package()).unwrap()
        );
        let mut stale = request.clone();
        stale["library_digest"] = json!("0".repeat(64));
        assert!(invoke(&tools, READ_RECIPE, stale).await.is_err());
        let mut omitted = request;
        omitted.as_object_mut().unwrap().remove("library_digest");
        assert!(invoke(&tools, READ_RECIPE, omitted).await.is_err());
    }
    assert_eq!(tools.tools().len(), 2);
    assert_eq!(
        tools.tool_mutation_class(READ_RECIPE),
        ToolMutationClass::ReadOnly
    );
}

#[tokio::test]
async fn changed_prerequisites_invalidate_library_pagination_snapshot() {
    use toolkit_package::{IntegrationBundle, IntegrationLibrary};
    const PLUGIN: &[u8] = include_bytes!(
        "../../toolkit-package/integrations/connectors/github-readonly/0.1.0/plugin.json"
    );
    const MCP: &[u8] = include_bytes!(
        "../../toolkit-package/integrations/connectors/github-readonly/0.1.0/mcp.json"
    );
    const RECIPES: &[u8] = include_bytes!(
        "../../toolkit-package/integrations/connectors/github-readonly/0.1.0/recipes.json"
    );
    let origin = CatalogOrigin::Curated {
        catalog: Id::new("test/library").unwrap(),
        revision: Revision::new("1").unwrap(),
    };
    let mut original = IntegrationLibrary::new(origin.clone());
    original
        .insert(IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap())
        .unwrap();
    let old_tools = DiscoveryTools::from_library(&original).unwrap();
    let page = invoke(&old_tools, LIST_RECIPES, json!({"limit":1}))
        .await
        .unwrap();
    let mut recipes: Value = serde_json::from_slice(RECIPES).unwrap();
    recipes["recipes"][0]["prerequisites"][0]["instruction"] = json!("Changed host requirement");
    let mut changed = IntegrationLibrary::new(origin);
    changed
        .insert(
            IntegrationBundle::parse(PLUGIN, Some(MCP), &serde_json::to_vec(&recipes).unwrap())
                .unwrap(),
        )
        .unwrap();
    let new_tools = DiscoveryTools::from_library(&changed).unwrap();
    assert!(
        invoke(
            &new_tools,
            LIST_RECIPES,
            json!({"limit":1,"snapshot":page["snapshot"]})
        )
        .await
        .is_err()
    );
    assert!(
        invoke(
            &old_tools,
            LIST_RECIPES,
            json!({"limit":1,"snapshot":page["snapshot"]})
        )
        .await
        .is_ok()
    );
}

async fn invoke(tool: &DiscoveryTools, name: &str, arguments: Value) -> Result<Value, ToolError> {
    let raw = RawValue::from_string(arguments.to_string()).unwrap();
    let outcome = tool
        .dispatch(ToolCallView {
            id: "call-a",
            name,
            args: &raw,
        })
        .await?;
    assert_eq!(outcome.result.tool_use_id, "call-a");
    assert!(!outcome.result.is_error);
    assert!(outcome.async_ops.is_empty());
    assert!(outcome.session_effects.is_empty());
    Ok(serde_json::from_str(&outcome.result.text_content()).unwrap())
}

#[tokio::test]
async fn every_bundled_recipe_can_be_read_exactly_across_pages() {
    let catalog = bundled_discovery_catalog().unwrap();
    let tool = DiscoveryTools::new(&catalog).unwrap();
    let mut request = json!({"limit":3});
    let mut read = Vec::new();
    loop {
        let page = invoke(&tool, LIST_RECIPES, request).await.unwrap();
        for summary in page["entries"].as_array().unwrap() {
            let entry = invoke(
                &tool,
                READ_RECIPE,
                json!({
                    "identity":summary["identity"],"digest":summary["digest"]
                }),
            )
            .await
            .unwrap();
            let identity = serde_json::from_value(summary["identity"].clone()).unwrap();
            assert_eq!(
                entry,
                serde_json::to_value(catalog.get(&identity).unwrap()).unwrap()
            );
            read.push(identity);
        }
        if page["next_after"].is_null() {
            break;
        }
        request = json!({"limit":3,"snapshot":page["snapshot"],"after":page["next_after"]});
    }
    assert_eq!(
        read,
        catalog
            .entries()
            .map(|e| e.package.identity.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(tool.tools().len(), 2);
    assert!(tool.tool_catalog_capabilities().exact_catalog);
    for name in [LIST_RECIPES, READ_RECIPE] {
        assert_eq!(tool.tool_mutation_class(name), ToolMutationClass::ReadOnly);
    }
    assert_eq!(
        tool.tool_mutation_class("install"),
        ToolMutationClass::Unknown
    );
}

#[tokio::test]
async fn cursors_and_digests_cannot_silently_resolve_other_recipe_content() {
    let original = bundled_discovery_catalog().unwrap();
    let tool = DiscoveryTools::new(&original).unwrap();
    let page = invoke(&tool, LIST_RECIPES, json!({"limit":1}))
        .await
        .unwrap();
    let first = &page["entries"][0];
    assert!(
        invoke(
            &tool,
            READ_RECIPE,
            json!({"identity":first["identity"],"digest":"0".repeat(64)})
        )
        .await
        .is_err()
    );
    assert!(
        invoke(
            &tool,
            LIST_RECIPES,
            json!({"limit":1,"after":first["identity"]})
        )
        .await
        .is_err()
    );
    let mut unknown = first["identity"].clone();
    unknown["name"] = json!("missing");
    assert!(
        invoke(
            &tool,
            LIST_RECIPES,
            json!({"limit":1,"snapshot":page["snapshot"],"after":unknown})
        )
        .await
        .is_err()
    );

    let mut changed = PackageCatalog::default();
    for entry in original.entries() {
        changed
            .insert(
                entry.package.clone(),
                CatalogOrigin::Curated {
                    catalog: Id::new("another-catalog").unwrap(),
                    revision: Revision::new("2").unwrap(),
                },
            )
            .unwrap();
    }
    let changed = DiscoveryTools::new(&changed).unwrap();
    assert!(
        invoke(
            &changed,
            LIST_RECIPES,
            json!({"limit":1,"snapshot":page["snapshot"],"after":first["identity"]})
        )
        .await
        .is_err()
    );
    // This adapter retains its own immutable inventory when the host builds a replacement.
    assert!(
        invoke(
            &tool,
            LIST_RECIPES,
            json!({"limit":1,"snapshot":page["snapshot"],"after":first["identity"]})
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn strict_requests_reject_authority_and_semantic_selection_fields() {
    let tool = DiscoveryTools::new(&bundled_discovery_catalog().unwrap()).unwrap();
    for request in [
        json!({"limit":0}),
        json!({"limit":21}),
        json!({"limit":1,"request":"choose email"}),
        json!({"limit":1,"authority":"admin"}),
        json!({"limit":1,"snapshot":"bad"}),
        json!([]),
    ] {
        assert!(invoke(&tool, LIST_RECIPES, request).await.is_err());
    }
    let page = invoke(&tool, LIST_RECIPES, json!({"limit":1}))
        .await
        .unwrap();
    let first = &page["entries"][0];
    let mut identity = first["identity"].clone();
    identity["authority"] = json!("admin");
    assert!(
        invoke(
            &tool,
            READ_RECIPE,
            json!({"identity":identity,"digest":first["digest"]})
        )
        .await
        .is_err()
    );
    assert!(matches!(
        invoke(&tool, "install", json!({})).await,
        Err(ToolError::NotFound { .. })
    ));
}

#[tokio::test]
async fn empty_inventory_is_a_terminal_page_and_oversized_metadata_is_refused() {
    let empty = DiscoveryTools::new(&PackageCatalog::default()).unwrap();
    let page = invoke(&empty, LIST_RECIPES, json!({"limit":20}))
        .await
        .unwrap();
    assert_eq!(page["entries"], json!([]));
    assert!(page["next_after"].is_null());

    let catalog = bundled_discovery_catalog().unwrap();
    let original = catalog.entries().next().unwrap();
    let mut package = original.package.clone();
    package.strategies[0].instructions = "x".repeat(1024 * 1024);
    let mut oversized = PackageCatalog::default();
    oversized.insert(package, original.origin.clone()).unwrap();
    assert!(matches!(
        DiscoveryTools::new(&oversized),
        Err(DiscoveryToolsError::MetadataTooLarge)
    ));
}

#[tokio::test]
async fn slack_successor_discovery_returns_both_guidance_routes_and_host_prerequisites() {
    let library = toolkit_package::bundled_integration_library().unwrap();
    let tools = DiscoveryTools::from_library(&library).unwrap();
    let page = invoke(&tools, LIST_RECIPES, json!({"limit":20}))
        .await
        .unwrap();
    let summary = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| {
            entry["identity"]["publisher"] == "meerkat-toolkit"
                && entry["identity"]["name"] == "slack-mcp-setup"
                && entry["identity"]["version"] == "0.2.0"
        })
        .unwrap();
    assert_eq!(summary["roles"], json!(["service_connector"]));
    let result = invoke(
        &tools,
        READ_RECIPE,
        json!({
            "identity":summary["identity"],
            "digest":summary["digest"],
            "library_digest":summary["library_digest"]
        }),
    )
    .await
    .unwrap();
    let strategies = result["package"]["strategies"].as_array().unwrap();
    assert_eq!(strategies.len(), 2);
    assert_eq!(strategies[0]["id"], "provider-mcp-setup");
    assert_eq!(strategies[1]["id"], "toolkit-rest-guidance");
    assert!(result["library"]["servers"].as_object().unwrap().is_empty());
    let recipes = result["library"]["document"]["recipes"].as_array().unwrap();
    assert_eq!(recipes.len(), 2);
    for (recipe, strategy) in recipes.iter().zip(strategies) {
        assert_eq!(recipe["strategy"], strategy["id"]);
        assert_eq!(recipe["role"], "service_connector");
        assert_eq!(recipe["plan"]["kind"], "guidance_only");
        assert!(!recipe["plan"]["blockers"].as_array().unwrap().is_empty());
        let prerequisites = recipe["prerequisites"].as_array().unwrap();
        for kind in [
            "developer_registration",
            "client_authentication",
            "application_binding",
        ] {
            assert!(prerequisites.iter().any(|p| p["kind"] == kind));
        }
    }
    assert!(
        recipes[1]["prerequisites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "artifact_preparation")
    );
}
