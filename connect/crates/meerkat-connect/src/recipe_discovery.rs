//! Read the canonical public recipe inventory before any service preparation.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolCatalogCapabilities, ToolDef, ToolError,
    ToolMutationClass, ToolProvenance, ToolResult, ToolSourceKind, ops::ToolDispatchOutcome,
};
use toolkit_package::{
    LIST_RECIPES, READ_RECIPE, RecipeInventory, bundled_recipe_catalog, recipe_tool_definitions,
};

/// Exact read-only tool names exposed by [`RecipeTools`].
pub const RECIPE_TOOLS: &[&str] = &[LIST_RECIPES, READ_RECIPE];

/// Native dispatcher for the canonical bundled public recipe inventory.
///
/// Records retain their service, frontend and hybrid roles and evidence. They
/// establish no installation, account authorization or runtime readiness. This
/// dispatcher performs no host, resource, credential or network operation.
///
/// The embedding host owns native admission and execution policy. For example,
/// a MobKit host can attach an `Arc<RecipeTools>` through its existing
/// `AgentBuildDraft::local_external_tools` customizer. Calling this dispatcher
/// directly does not itself enforce a caller policy. The inventory is loaded
/// once, on the first dispatched read; no session or runtime is created.
pub struct RecipeTools {
    inventory: OnceLock<Result<RecipeInventory, ()>>,
    definitions: Arc<[Arc<ToolDef>]>,
}

impl RecipeTools {
    /// Construct the bundled reader without loading resources or pairing a host.
    pub fn new() -> Self {
        Self {
            inventory: OnceLock::new(),
            definitions: recipe_tool_definitions()
                .into_iter()
                .map(|definition| {
                    Arc::new(
                        ToolDef::new(
                            definition.name,
                            definition.description,
                            definition.input_schema,
                        )
                        .with_provenance(ToolProvenance::new(
                            ToolSourceKind::Callback,
                            "meerkat-connect:recipe-inventory",
                        )),
                    )
                })
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

impl Default for RecipeTools {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid(name: &str) -> ToolError {
    ToolError::invalid_arguments(
        name,
        "Use the exact recipe identity, digest, library_digest when listed, and inventory cursor returned by these tools. List limit must be 1-20; restart listing if its snapshot changed.",
    )
}

#[async_trait]
impl AgentToolDispatcher for RecipeTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.definitions.clone()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if RECIPE_TOOLS.contains(&name) {
            ToolMutationClass::ReadOnly
        } else {
            ToolMutationClass::Unknown
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !RECIPE_TOOLS.contains(&call.name) {
            return Err(ToolError::not_found(call.name));
        }
        if call.id.is_empty() || call.id.len() > 512 || call.args.get().len() > 8192 {
            return Err(invalid(call.name));
        }
        // Immutable embedded data only. A native host applies its caller
        // admission before dispatch; direct callers own that boundary.
        // An invalid bundled inventory cannot fall back to a partial catalog.
        let inventory = self
            .inventory
            .get_or_init(|| {
                let catalog = bundled_recipe_catalog().map_err(|_| ())?;
                RecipeInventory::new(&catalog).map_err(|_| ())
            })
            .as_ref()
            .map_err(|_| ToolError::execution_failed("Bundled recipe metadata is unavailable"))?;
        let encoded = match call.name {
            LIST_RECIPES => serde_json::to_string(
                &inventory
                    .list(call.parse_args().map_err(|_| invalid(call.name))?)
                    .map_err(|_| invalid(call.name))?,
            ),
            READ_RECIPE => serde_json::to_string(
                &inventory
                    .read(call.parse_args().map_err(|_| invalid(call.name))?)
                    .map_err(|_| invalid(call.name))?,
            ),
            _ => unreachable!(),
        }
        .map_err(|_| invalid(call.name))?;
        Ok(ToolResult::new(call.id.into(), encoded, false).into())
    }
}
