//! Compile the exact inert candidate adapter against released native contracts.
//! This does not import or qualify its unpublished host/runtime workspace.

#[path = "../../toolkit-mobkit/src/discovery.rs"]
mod candidate;

use candidate::{DiscoveryTools, DiscoveryToolsError, LIST_RECIPES, READ_RECIPE};
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolError, ToolMutationClass};
use serde_json::{Value, json};
use toolkit_package::{RecipeInventory, bundled_integration_library, bundled_recipe_catalog};

async fn invoke(tool: &DiscoveryTools, name: &str, arguments: Value) -> Result<String, ToolError> {
    let raw = serde_json::value::to_raw_value(&arguments).unwrap();
    let outcome = tool
        .dispatch(ToolCallView {
            id: "candidate-probe",
            name,
            args: &raw,
        })
        .await?;
    assert_eq!(outcome.result.tool_use_id, "candidate-probe");
    assert!(!outcome.result.is_error);
    assert!(outcome.async_ops.is_empty() && outcome.session_effects.is_empty());
    Ok(outcome.result.text_content())
}

#[tokio::test]
async fn exact_candidate_adapter_preserves_catalog_library_wire_and_selector_errors() {
    let catalog = bundled_recipe_catalog().unwrap();
    let library = bundled_integration_library().unwrap();
    let direct: Result<DiscoveryTools, DiscoveryToolsError> = DiscoveryTools::new(&catalog);
    for (tool, inventory) in [
        (direct.unwrap(), RecipeInventory::new(&catalog).unwrap()),
        (
            DiscoveryTools::from_library(&library).unwrap(),
            RecipeInventory::from_library(&library).unwrap(),
        ),
    ] {
        assert_eq!(
            tool.tools()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [LIST_RECIPES, READ_RECIPE]
        );
        assert!(tool.tool_catalog_capabilities().exact_catalog);
        assert!(
            !tool
                .tool_catalog_capabilities()
                .may_require_catalog_control_plane
        );
        for name in [LIST_RECIPES, READ_RECIPE] {
            assert_eq!(tool.tool_mutation_class(name), ToolMutationClass::ReadOnly);
        }
        let request = json!({"limit":1});
        let encoded = invoke(&tool, LIST_RECIPES, request.clone()).await.unwrap();
        assert_eq!(
            encoded,
            serde_json::to_string(
                &inventory
                    .list(serde_json::from_value(request).unwrap())
                    .unwrap()
            )
            .unwrap()
        );
        let page: Value = serde_json::from_str(&encoded).unwrap();
        let summary = &page["entries"][0];
        assert!(summary.get("origin").is_none() && summary.get("assessments").is_none());
        let named_request = json!({"limit":20,"name":summary["identity"]["name"]});
        let named = invoke(&tool, LIST_RECIPES, named_request.clone())
            .await
            .unwrap();
        assert_eq!(
            named,
            serde_json::to_string(
                &inventory
                    .list(serde_json::from_value(named_request).unwrap())
                    .unwrap()
            )
            .unwrap()
        );
        let named: Value = serde_json::from_str(&named).unwrap();
        assert!(
            named["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| { entry["identity"]["name"] == summary["identity"]["name"] })
        );
        assert_ne!(named["snapshot"], page["snapshot"]);
        let mut exact = json!({"identity":summary["identity"],"digest":summary["digest"]});
        if let Some(digest) = summary.get("library_digest") {
            exact["library_digest"] = digest.clone();
        }
        assert_eq!(
            invoke(&tool, READ_RECIPE, exact.clone()).await.unwrap(),
            serde_json::to_string(
                &inventory
                    .read(serde_json::from_value(exact.clone()).unwrap())
                    .unwrap()
            )
            .unwrap()
        );
        let mut stale = exact;
        stale["digest"] = json!("0".repeat(64));
        for (name, arguments) in [
            (LIST_RECIPES, json!({"limit":0})),
            (LIST_RECIPES, json!({"limit":1,"snapshot":"0".repeat(64)})),
            (
                LIST_RECIPES,
                json!({"limit":1,"snapshot":named["snapshot"]}),
            ),
            (READ_RECIPE, stale),
        ] {
            assert!(matches!(invoke(&tool, name, arguments).await,
                Err(ToolError::InvalidArguments { name: rejected, .. }) if rejected == name));
        }
        assert!(matches!(
            invoke(&tool, "install", json!({})).await,
            Err(ToolError::NotFound { .. })
        ));
    }
}
