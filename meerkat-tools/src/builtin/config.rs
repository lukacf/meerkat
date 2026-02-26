//! Tool policy configuration for built-in tools
//!
//! This module provides [`ToolPolicyLayer`] for configuring which tools are enabled/disabled,
//! and [`EnforcedToolPolicy`] for hard constraints that cannot be overridden.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Tool enable/disable mode
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    /// All tools enabled by default
    #[default]
    AllowAll,
    /// All tools disabled by default
    DenyAll,
    /// Only explicitly allowed tools enabled
    AllowList,
}

/// A single layer of tool policy configuration.
/// Multiple layers are merged with later layers taking precedence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolPolicyLayer {
    /// Base mode for this layer
    #[serde(default)]
    pub mode: Option<ToolMode>,

    /// Tools to enable (soft - can be overridden by later layers)
    #[serde(default)]
    pub enable: HashSet<String>,

    /// Tools to disable (soft - can be overridden by later layers)
    #[serde(default)]
    pub disable: HashSet<String>,
}

/// Enforced policy that cannot be overridden by later layers.
/// Applied after all soft policies are merged.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnforcedToolPolicy {
    /// Hard allowlist - if non-empty, only these tools can be enabled
    #[serde(default)]
    pub allow: HashSet<String>,

    /// Hard denylist - these tools are always disabled
    #[serde(default)]
    pub deny: HashSet<String>,
}

/// Complete builtin tools configuration
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuiltinToolConfig {
    /// Soft policy layers (merged in order)
    #[serde(default)]
    pub policy: ToolPolicyLayer,

    /// Enforced policy (applied last, cannot be overridden)
    #[serde(default)]
    pub enforced: EnforcedToolPolicy,
}

impl ToolPolicyLayer {
    /// Create a new empty policy layer
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the mode for this layer
    pub fn with_mode(mut self, mode: ToolMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Add a tool to the enable set
    pub fn enable_tool(mut self, name: impl Into<String>) -> Self {
        self.enable.insert(name.into());
        self
    }

    /// Add a tool to the disable set
    pub fn disable_tool(mut self, name: impl Into<String>) -> Self {
        self.disable.insert(name.into());
        self
    }
}

/// Resolved tool policy after merging all layers
#[derive(Clone, Debug)]
pub struct ResolvedToolPolicy {
    mode: ToolMode,
    enabled: HashSet<String>,
    disabled: HashSet<String>,
}

impl ResolvedToolPolicy {
    /// Check if a tool is enabled given its default state
    pub fn is_enabled(&self, tool_name: &str, default_enabled: bool) -> bool {
        // First check enforced policy (applied by resolve_with_enforced)
        if self.disabled.contains(tool_name) {
            return false;
        }
        if self.enabled.contains(tool_name) {
            return true;
        }

        // Then check mode
        match self.mode {
            ToolMode::AllowAll => default_enabled,
            ToolMode::DenyAll | ToolMode::AllowList => false,
        }
    }

    /// Get the resolved mode
    pub fn mode(&self) -> &ToolMode {
        &self.mode
    }

    /// Get the set of explicitly enabled tools
    pub fn enabled(&self) -> &HashSet<String> {
        &self.enabled
    }

    /// Get the set of explicitly disabled tools
    pub fn disabled(&self) -> &HashSet<String> {
        &self.disabled
    }
}

/// Merge multiple policy layers, later layers take precedence
pub fn merge_policy_layers(layers: &[ToolPolicyLayer]) -> ResolvedToolPolicy {
    let mut mode = ToolMode::AllowAll;
    let mut tool_states: HashMap<String, bool> = HashMap::new();

    for layer in layers {
        if let Some(m) = &layer.mode {
            mode = m.clone();
        }
        for tool in &layer.enable {
            tool_states.insert(tool.clone(), true);
        }
        for tool in &layer.disable {
            tool_states.insert(tool.clone(), false);
        }
    }

    let enabled: HashSet<String> = tool_states
        .iter()
        .filter(|(_, v)| **v)
        .map(|(k, _)| k.clone())
        .collect();
    let disabled: HashSet<String> = tool_states
        .iter()
        .filter(|(_, v)| !**v)
        .map(|(k, _)| k.clone())
        .collect();

    ResolvedToolPolicy {
        mode,
        enabled,
        disabled,
    }
}

/// Apply enforced policy on top of resolved policy
pub fn apply_enforced_policy(
    mut resolved: ResolvedToolPolicy,
    enforced: &EnforcedToolPolicy,
) -> ResolvedToolPolicy {
    // Hard denylist always wins
    for tool in &enforced.deny {
        resolved.disabled.insert(tool.clone());
        resolved.enabled.remove(tool);
    }

    // If allowlist is non-empty, only those tools can be enabled
    if !enforced.allow.is_empty() {
        resolved.enabled.retain(|t| enforced.allow.contains(t));
        // Any tool not in allow list is implicitly disabled
        // (handled by is_enabled returning false for non-enabled tools in AllowList mode)
        resolved.mode = ToolMode::AllowList;
    }

    resolved
}

impl BuiltinToolConfig {
    /// Resolve config to final policy
    pub fn resolve(&self) -> ResolvedToolPolicy {
        let resolved = merge_policy_layers(std::slice::from_ref(&self.policy));
        apply_enforced_policy(resolved, &self.enforced)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_mode_serde() {
        // Test serialization
        let allow_all = ToolMode::AllowAll;
        let json = serde_json::to_string(&allow_all).unwrap();
        assert_eq!(json, "\"allow_all\"");

        let deny_all = ToolMode::DenyAll;
        let json = serde_json::to_string(&deny_all).unwrap();
        assert_eq!(json, "\"deny_all\"");

        let allow_list = ToolMode::AllowList;
        let json = serde_json::to_string(&allow_list).unwrap();
        assert_eq!(json, "\"allow_list\"");

        // Test deserialization
        let mode: ToolMode = serde_json::from_str("\"allow_all\"").unwrap();
        assert_eq!(mode, ToolMode::AllowAll);

        let mode: ToolMode = serde_json::from_str("\"deny_all\"").unwrap();
        assert_eq!(mode, ToolMode::DenyAll);

        let mode: ToolMode = serde_json::from_str("\"allow_list\"").unwrap();
        assert_eq!(mode, ToolMode::AllowList);
    }

    #[test]
    fn test_tool_policy_layer_serde() {
        // Test with all fields populated
        let layer = ToolPolicyLayer {
            mode: Some(ToolMode::DenyAll),
            enable: ["read_file", "write_file"]
                .into_iter()
                .map(String::from)
                .collect(),
            disable: ["bash"].into_iter().map(String::from).collect(),
        };

        let json = serde_json::to_string(&layer).unwrap();
        let deserialized: ToolPolicyLayer = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.mode, Some(ToolMode::DenyAll));
        assert!(deserialized.enable.contains("read_file"));
        assert!(deserialized.enable.contains("write_file"));
        assert!(deserialized.disable.contains("bash"));

        // Test with empty/default values
        let empty_layer: ToolPolicyLayer = serde_json::from_str("{}").unwrap();
        assert_eq!(empty_layer.mode, None);
        assert!(empty_layer.enable.is_empty());
        assert!(empty_layer.disable.is_empty());
    }

    #[test]
    fn test_enforced_policy_serde() {
        let policy = EnforcedToolPolicy {
            allow: ["read_file", "list_dir"]
                .into_iter()
                .map(String::from)
                .collect(),
            deny: ["bash", "write_file"]
                .into_iter()
                .map(String::from)
                .collect(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: EnforcedToolPolicy = serde_json::from_str(&json).unwrap();

        assert!(deserialized.allow.contains("read_file"));
        assert!(deserialized.allow.contains("list_dir"));
        assert!(deserialized.deny.contains("bash"));
        assert!(deserialized.deny.contains("write_file"));

        // Test with empty/default values
        let empty_policy: EnforcedToolPolicy = serde_json::from_str("{}").unwrap();
        assert!(empty_policy.allow.is_empty());
        assert!(empty_policy.deny.is_empty());
    }

    #[test]
    fn test_builtin_tool_config_serde() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer {
                mode: Some(ToolMode::AllowList),
                enable: ["read_file"].into_iter().map(String::from).collect(),
                disable: HashSet::new(),
            },
            enforced: EnforcedToolPolicy {
                allow: HashSet::new(),
                deny: ["bash"].into_iter().map(String::from).collect(),
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BuiltinToolConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.policy.mode, Some(ToolMode::AllowList));
        assert!(deserialized.policy.enable.contains("read_file"));
        assert!(deserialized.enforced.deny.contains("bash"));

        // Test with empty/default values
        let empty_config: BuiltinToolConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty_config.policy.mode, None);
        assert!(empty_config.policy.enable.is_empty());
        assert!(empty_config.enforced.allow.is_empty());
    }

    #[test]
    fn test_policy_layer_builder_methods() {
        let layer = ToolPolicyLayer::new()
            .with_mode(ToolMode::DenyAll)
            .enable_tool("read_file")
            .enable_tool("write_file")
            .disable_tool("bash");

        assert_eq!(layer.mode, Some(ToolMode::DenyAll));
        assert!(layer.enable.contains("read_file"));
        assert!(layer.enable.contains("write_file"));
        assert_eq!(layer.enable.len(), 2);
        assert!(layer.disable.contains("bash"));
        assert_eq!(layer.disable.len(), 1);
    }

    #[test]
    fn test_tool_mode_default() {
        let mode = ToolMode::default();
        assert_eq!(mode, ToolMode::AllowAll);
    }

    #[test]
    fn test_tool_policy_layer_default() {
        let layer = ToolPolicyLayer::default();
        assert_eq!(layer.mode, None);
        assert!(layer.enable.is_empty());
        assert!(layer.disable.is_empty());
    }

    #[test]
    fn test_enforced_policy_default() {
        let policy = EnforcedToolPolicy::default();
        assert!(policy.allow.is_empty());
        assert!(policy.deny.is_empty());
    }

    #[test]
    fn test_builtin_tool_config_default() {
        let config = BuiltinToolConfig::default();
        assert_eq!(config.policy.mode, None);
        assert!(config.policy.enable.is_empty());
        assert!(config.enforced.allow.is_empty());
    }

    // ==================== Resolution Tests ====================

    #[test]
    fn test_merge_empty_layers() {
        let resolved = merge_policy_layers(&[]);
        assert_eq!(*resolved.mode(), ToolMode::AllowAll);
        assert!(resolved.enabled().is_empty());
        assert!(resolved.disabled().is_empty());
    }

    #[test]
    fn test_merge_single_layer_enable() {
        let layer = ToolPolicyLayer::new()
            .enable_tool("read_file")
            .enable_tool("write_file");
        let resolved = merge_policy_layers(&[layer]);

        assert_eq!(*resolved.mode(), ToolMode::AllowAll);
        assert!(resolved.enabled().contains("read_file"));
        assert!(resolved.enabled().contains("write_file"));
        assert_eq!(resolved.enabled().len(), 2);
        assert!(resolved.disabled().is_empty());
    }

    #[test]
    fn test_merge_single_layer_disable() {
        let layer = ToolPolicyLayer::new()
            .disable_tool("bash")
            .disable_tool("shell");
        let resolved = merge_policy_layers(&[layer]);

        assert_eq!(*resolved.mode(), ToolMode::AllowAll);
        assert!(resolved.disabled().contains("bash"));
        assert!(resolved.disabled().contains("shell"));
        assert_eq!(resolved.disabled().len(), 2);
        assert!(resolved.enabled().is_empty());
    }

    #[test]
    fn test_merge_later_layer_wins() {
        // First layer enables "bash", second layer disables it
        let layer1 = ToolPolicyLayer::new().enable_tool("bash");
        let layer2 = ToolPolicyLayer::new().disable_tool("bash");
        let resolved = merge_policy_layers(&[layer1, layer2]);

        assert!(resolved.disabled().contains("bash"));
        assert!(!resolved.enabled().contains("bash"));

        // Reverse order: first disables, second enables
        let layer1 = ToolPolicyLayer::new().disable_tool("bash");
        let layer2 = ToolPolicyLayer::new().enable_tool("bash");
        let resolved = merge_policy_layers(&[layer1, layer2]);

        assert!(resolved.enabled().contains("bash"));
        assert!(!resolved.disabled().contains("bash"));
    }

    #[test]
    fn test_merge_mode_override() {
        // First layer sets AllowAll, second sets DenyAll
        let layer1 = ToolPolicyLayer::new().with_mode(ToolMode::AllowAll);
        let layer2 = ToolPolicyLayer::new().with_mode(ToolMode::DenyAll);
        let resolved = merge_policy_layers(&[layer1, layer2]);

        assert_eq!(*resolved.mode(), ToolMode::DenyAll);

        // Layer without mode doesn't change existing mode
        let layer1 = ToolPolicyLayer::new().with_mode(ToolMode::DenyAll);
        let layer2 = ToolPolicyLayer::new().enable_tool("read_file"); // no mode set
        let resolved = merge_policy_layers(&[layer1, layer2]);

        assert_eq!(*resolved.mode(), ToolMode::DenyAll);
    }

    #[test]
    fn test_enforced_deny_overrides_enable() {
        let layer = ToolPolicyLayer::new()
            .enable_tool("bash")
            .enable_tool("read_file");
        let resolved = merge_policy_layers(&[layer]);

        // bash is enabled after merge
        assert!(resolved.enabled().contains("bash"));

        // Apply enforced policy that denies bash
        let enforced = EnforcedToolPolicy {
            allow: HashSet::new(),
            deny: ["bash"].into_iter().map(String::from).collect(),
        };
        let resolved = apply_enforced_policy(resolved, &enforced);

        // bash should now be disabled, read_file still enabled
        assert!(resolved.disabled().contains("bash"));
        assert!(!resolved.enabled().contains("bash"));
        assert!(resolved.enabled().contains("read_file"));
    }

    #[test]
    fn test_enforced_allow_restricts_to_allowlist() {
        let layer = ToolPolicyLayer::new()
            .enable_tool("bash")
            .enable_tool("read_file")
            .enable_tool("write_file");
        let resolved = merge_policy_layers(&[layer]);

        // All three are enabled
        assert_eq!(resolved.enabled().len(), 3);

        // Apply enforced allowlist that only permits read_file
        let enforced = EnforcedToolPolicy {
            allow: ["read_file"].into_iter().map(String::from).collect(),
            deny: HashSet::new(),
        };
        let resolved = apply_enforced_policy(resolved, &enforced);

        // Only read_file should remain enabled
        assert!(resolved.enabled().contains("read_file"));
        assert!(!resolved.enabled().contains("bash"));
        assert!(!resolved.enabled().contains("write_file"));
        assert_eq!(resolved.enabled().len(), 1);

        // Mode should be AllowList
        assert_eq!(*resolved.mode(), ToolMode::AllowList);
    }

    #[test]
    fn test_is_enabled_with_default_true() {
        // AllowAll mode: default_enabled=true means enabled
        let resolved = merge_policy_layers(&[]);
        assert!(resolved.is_enabled("any_tool", true));

        // DenyAll mode: default doesn't matter, always false for non-explicit
        let layer = ToolPolicyLayer::new().with_mode(ToolMode::DenyAll);
        let resolved = merge_policy_layers(&[layer]);
        assert!(!resolved.is_enabled("any_tool", true));

        // AllowList mode: default doesn't matter, must be explicit
        let layer = ToolPolicyLayer::new().with_mode(ToolMode::AllowList);
        let resolved = merge_policy_layers(&[layer]);
        assert!(!resolved.is_enabled("any_tool", true));
    }

    #[test]
    fn test_is_enabled_with_default_false() {
        // AllowAll mode: default_enabled=false means disabled
        let resolved = merge_policy_layers(&[]);
        assert!(!resolved.is_enabled("any_tool", false));

        // Explicitly enabled tool should be enabled regardless of default
        let layer = ToolPolicyLayer::new().enable_tool("my_tool");
        let resolved = merge_policy_layers(&[layer]);
        assert!(resolved.is_enabled("my_tool", false));

        // Explicitly disabled tool should be disabled regardless of default
        let layer = ToolPolicyLayer::new().disable_tool("my_tool");
        let resolved = merge_policy_layers(&[layer]);
        assert!(!resolved.is_enabled("my_tool", true));
    }

    #[test]
    fn test_builtin_tool_config_resolve() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .with_mode(ToolMode::DenyAll)
                .enable_tool("read_file")
                .enable_tool("bash"),
            enforced: EnforcedToolPolicy {
                allow: HashSet::new(),
                deny: ["bash"].into_iter().map(String::from).collect(),
            },
        };

        let resolved = config.resolve();

        // read_file is enabled (in policy, not in enforced deny)
        assert!(resolved.is_enabled("read_file", false));

        // bash is disabled (enforced deny overrides policy enable)
        assert!(!resolved.is_enabled("bash", false));

        // other tools follow mode (DenyAll)
        assert!(!resolved.is_enabled("write_file", true));
    }
}
