//! Optional read-only access to an explicitly supplied public recipe inventory.
//! Recipes are untrusted investigation evidence, never execution authority.

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolCatalogCapabilities, ToolDef, ToolError,
    ToolMutationClass, ToolResult, ops::ToolDispatchOutcome,
};
use std::sync::Arc;
use toolkit_package::{
    IntegrationLibrary, PackageCatalog, RecipeInventory, recipe_tool_definitions,
};
pub use toolkit_package::{LIST_RECIPES, READ_RECIPE, RecipeInventoryError as DiscoveryToolsError};

/// The host must supply only public metadata suitable for every selected agent.
/// Private catalogs require their own authenticated read boundary. This adapter
/// has no network, filesystem, credential, installation or package-choice path.
pub struct DiscoveryTools {
    inventory: RecipeInventory,
    tools: Arc<[Arc<ToolDef>]>,
}

impl DiscoveryTools {
    pub fn new(public_catalog: &PackageCatalog) -> Result<Self, DiscoveryToolsError> {
        Ok(Self {
            inventory: RecipeInventory::new(public_catalog)?,
            tools: definitions(),
        })
    }

    /// Expose retained configurations and typed prerequisites through the same
    /// read-only discovery tools. Hosts still explicitly choose this inventory.
    pub fn from_library(library: &IntegrationLibrary) -> Result<Self, DiscoveryToolsError> {
        Ok(Self {
            inventory: RecipeInventory::from_library(library)?,
            tools: definitions(),
        })
    }
}

fn invalid(name: &str) -> ToolError {
    ToolError::invalid_arguments(
        name,
        "Use the exact recipe identity, digest, library_digest when listed, and inventory cursor returned by these tools. List limit must be 1-20; restart listing if its snapshot changed.",
    )
}

#[async_trait]
impl AgentToolDispatcher for DiscoveryTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        match name {
            LIST_RECIPES | READ_RECIPE => ToolMutationClass::ReadOnly,
            _ => ToolMutationClass::Unknown,
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !matches!(call.name, LIST_RECIPES | READ_RECIPE) {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        if call.id.is_empty() || call.id.len() > 512 || call.args.get().len() > 8192 {
            return Err(invalid(call.name));
        }
        let encoded = match call.name {
            LIST_RECIPES => {
                let request = call.parse_args().map_err(|_| invalid(call.name))?;
                serde_json::to_string(
                    &self
                        .inventory
                        .list(request)
                        .map_err(|_| invalid(call.name))?,
                )
            }
            READ_RECIPE => {
                let request = call.parse_args().map_err(|_| invalid(call.name))?;
                serde_json::to_string(
                    &self
                        .inventory
                        .read(request)
                        .map_err(|_| invalid(call.name))?,
                )
            }
            _ => unreachable!(),
        }
        .map_err(|_| invalid(call.name))?;
        Ok(ToolResult::new(call.id.into(), encoded, false).into())
    }
}

fn definitions() -> Arc<[Arc<ToolDef>]> {
    recipe_tool_definitions()
        .into_iter()
        .map(|definition| {
            Arc::new(ToolDef::new(
                definition.name,
                definition.description,
                definition.input_schema,
            ))
        })
        .collect::<Vec<_>>()
        .into()
}
