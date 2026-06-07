//! Tool registry for tracking and validating tools

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::ToolValidationError;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::types::{ToolDef, ToolIdentity, ToolName};

/// Registry for managing available tools and their schemas
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<ToolName, Arc<ToolDef>>,
}

/// Typed lifecycle state of an admitted tool identity.
///
/// The registry remembers tools across dynamic catalog refreshes so a tool that
/// is momentarily uncallable still surfaces as registered. A tool that vanishes
/// from the router's live catalog entirely, however, must NOT continue to be
/// published as catalog truth: it is `Retired` and treated as not-present
/// (dispatch returns `NotFound`, the admitted snapshot omits it). This
/// distinguishes a genuinely-registered-but-temporarily-unavailable tool (which
/// remains in the live catalog carrying its own unavailable callability) from a
/// vanished tool (gone from the source set), instead of fabricating
/// `NotCurrentlyCallable` for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAvailability {
    /// The tool is present in the most recent live catalog reconciliation.
    Live,
    /// The tool was previously admitted but the live catalog no longer claims
    /// it. It is retained as a typed tombstone for diagnostics but is not
    /// treated as a present/callable tool.
    Retired,
}

/// Registry of admitted tool identities in catalog order.
///
/// This intentionally records only identity plus the static tool definition
/// needed for validation/fallback projections. Dispatchers refresh it as
/// dynamic catalogs change and reconcile against the live catalog so vanished
/// tools are retired; live callability remains owned by the router catalog.
#[derive(Debug, Clone, Default)]
pub struct ToolIdentityRegistry {
    entries: Vec<ToolIdentityEntry>,
    by_name: HashMap<ToolName, usize>,
}

#[derive(Debug, Clone)]
pub struct ToolIdentityEntry {
    pub identity: ToolIdentity,
    pub tool: Arc<ToolDef>,
    pub availability: ToolAvailability,
}

impl ToolIdentityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_catalog(catalog: &[ToolCatalogEntry]) -> Self {
        let mut registry = Self::new();
        for entry in catalog {
            registry.register(Arc::clone(&entry.tool));
        }
        registry
    }

    pub fn register(&mut self, tool: Arc<ToolDef>) {
        let identity = tool.identity();
        if let Some(index) = self.by_name.get(&identity.name).copied() {
            self.entries[index] = ToolIdentityEntry {
                identity,
                tool,
                availability: ToolAvailability::Live,
            };
            return;
        }
        let index = self.entries.len();
        self.by_name.insert(identity.name.clone(), index);
        self.entries.push(ToolIdentityEntry {
            identity,
            tool,
            availability: ToolAvailability::Live,
        });
    }

    /// Reconcile the registry against the names the live catalog currently
    /// claims: any admitted entry absent from `live_names` is marked `Retired`
    /// (it vanished from the source set). Entries present in `live_names` are
    /// (re)marked `Live`, so a tool that reappears after a transient dropout is
    /// promoted back to live.
    pub fn reconcile_to_live<'a>(&mut self, live_names: impl IntoIterator<Item = &'a str>) {
        let live: std::collections::BTreeSet<&str> = live_names.into_iter().collect();
        for entry in &mut self.entries {
            entry.availability = if live.contains(entry.identity.name.as_str()) {
                ToolAvailability::Live
            } else {
                ToolAvailability::Retired
            };
        }
    }

    /// True only when `name` is admitted AND still live (not retired/vanished).
    pub fn contains(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|entry| entry.availability == ToolAvailability::Live)
    }

    /// Fetch a live entry by name. Retired (vanished) entries are not returned.
    pub fn get(&self, name: &str) -> Option<&ToolIdentityEntry> {
        self.by_name
            .get(name)
            .and_then(|index| self.entries.get(*index))
            .filter(|entry| entry.availability == ToolAvailability::Live)
    }

    /// Iterate live entries only. Retired tombstones are skipped so vanished
    /// tools are never republished as catalog truth.
    pub fn iter(&self) -> impl Iterator<Item = &ToolIdentityEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.availability == ToolAvailability::Live)
    }

    pub fn len(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.availability == ToolAvailability::Live)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new tool
    pub fn register(&mut self, def: ToolDef) {
        self.tools.insert(def.tool_name(), Arc::new(def));
    }

    /// Register multiple tools
    pub fn register_many(&mut self, defs: impl IntoIterator<Item = ToolDef>) {
        for def in defs {
            self.register(def);
        }
    }

    /// Get a tool definition by name
    pub fn get(&self, name: &str) -> Option<Arc<ToolDef>> {
        self.tools.get(name).cloned()
    }

    /// Get all registered tools
    pub fn list(&self) -> Vec<Arc<ToolDef>> {
        self.tools.values().cloned().collect()
    }

    /// Validate tool arguments against its schema
    pub fn validate(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<(), ToolValidationError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolValidationError::not_found(name))?;

        Self::validate_tool_def(tool.as_ref(), name, args)
    }

    /// Validate tool arguments against a specific live tool definition.
    pub fn validate_tool_def(
        tool: &ToolDef,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<(), ToolValidationError> {
        // Basic schema validation using jsonschema
        let compiled = jsonschema::Validator::new(&tool.input_schema)
            .map_err(|e| ToolValidationError::invalid_arguments(name, e.to_string()))?;

        if let Err(error) = compiled.validate(args) {
            return Err(ToolValidationError::invalid_arguments(
                name,
                error.to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::schema::empty_object_schema;
    use serde_json::json;

    fn test_tool(name: &str, description: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.into(),
            description: description.to_string(),
            input_schema: empty_object_schema(),
            provenance: None,
        })
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        let def = ToolDef {
            name: "test_tool".into(),
            description: "A test tool".to_string(),
            input_schema: empty_object_schema(),
            provenance: None,
        };

        registry.register(def.clone());
        let fetched = registry.get("test_tool").unwrap();
        assert_eq!(fetched.name, def.name);
    }

    #[test]
    fn test_registry_validate_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.validate("missing", &json!({}));
        assert!(matches!(result, Err(ToolValidationError::NotFound { .. })));
    }

    #[test]
    fn test_registry_validate_invalid_args() {
        let mut registry = ToolRegistry::new();
        let def = ToolDef {
            name: "test_tool".into(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer" }
                },
                "required": ["count"]
            }),
            provenance: None,
        };

        registry.register(def);

        // Missing required field
        let result = registry.validate("test_tool", &json!({}));
        assert!(matches!(
            result,
            Err(ToolValidationError::InvalidArguments { .. })
        ));

        // Wrong type
        let result = registry.validate("test_tool", &json!({"count": "not a number"}));
        assert!(matches!(
            result,
            Err(ToolValidationError::InvalidArguments { .. })
        ));

        // Valid args
        let result = registry.validate("test_tool", &json!({"count": 42}));
        assert!(result.is_ok());
    }

    #[test]
    fn identity_registry_admits_late_identities_and_updates_existing_definitions() {
        let mut registry = ToolIdentityRegistry::new();

        registry.register(test_tool("initial", "old initial schema"));
        registry.register(test_tool("late", "late schema"));
        registry.register(test_tool("initial", "new initial schema"));

        let names: Vec<_> = registry
            .iter()
            .map(|entry| entry.identity.name.to_string())
            .collect();
        assert_eq!(names, vec!["initial".to_string(), "late".to_string()]);
        assert_eq!(
            registry.get("initial").unwrap().tool.description,
            "new initial schema"
        );
        assert!(registry.contains("late"));
    }

    #[test]
    fn identity_registry_retires_vanished_tools_on_reconcile() {
        // Dogma row #352: a tool removed from the live catalog must be retired,
        // not republished as catalog truth. After reconcile, only the still-live
        // tool is visible/contained; the vanished one is treated as not-present.
        let mut registry = ToolIdentityRegistry::new();
        registry.register(test_tool("initial", "schema"));
        registry.register(test_tool("late", "schema"));

        // "initial" vanishes from the source set; only "late" remains live.
        registry.reconcile_to_live(["late"]);

        assert!(
            !registry.contains("initial"),
            "vanished tool must not be reported as present"
        );
        assert!(registry.get("initial").is_none());
        assert!(registry.contains("late"));

        let live_names: Vec<_> = registry
            .iter()
            .map(|entry| entry.identity.name.to_string())
            .collect();
        assert_eq!(live_names, vec!["late".to_string()]);
        assert_eq!(registry.len(), 1);

        // A tool that reappears in the live catalog is promoted back to live.
        registry.reconcile_to_live(["initial", "late"]);
        assert!(registry.contains("initial"));
        assert_eq!(registry.len(), 2);
    }
}
