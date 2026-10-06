//! Explicit host supply of public MCP descriptors to child mob profiles.

use std::collections::BTreeMap;

use meerkat_core::McpServerConfig;
use meerkat_mob::{MobDefinition, MobError};

use crate::ChildToolBundleAvailability;

/// A conflicting destination may not silently replace an existing descriptor.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChildMcpServerRegistrationError {
    #[error("conflicting child MCP server registration: {name}")]
    ConflictingServer { name: String },
}

/// Host-attested public MCP descriptors. Registration attests that the entire
/// config may be persisted with child profiles, including command, arguments,
/// environment, URL, and headers. Never register runtime credentials here.
/// A host's process-local connection resolver supplies the protected config.
///
/// Availability controls automatic supply only. The resolver must still deny
/// unauthorized callers that copy a host-only public descriptor themselves.
#[derive(Clone, Default)]
pub struct ChildMcpServers {
    servers: BTreeMap<String, (McpServerConfig, ChildToolBundleAvailability)>,
}

impl std::fmt::Debug for ChildMcpServers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_map()
            .entries(
                self.servers
                    .iter()
                    .map(|(name, (_, availability))| (name, availability)),
            )
            .finish()
    }
}

impl ChildMcpServers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a public descriptor, explicitly attesting its suitability for
    /// persistence. Re-registering the exact descriptor may change availability.
    pub fn register(
        mut self,
        config: McpServerConfig,
        availability: ChildToolBundleAvailability,
    ) -> Result<Self, ChildMcpServerRegistrationError> {
        if self
            .servers
            .get(&config.name)
            .is_some_and(|(existing, _)| existing != &config)
        {
            return Err(ChildMcpServerRegistrationError::ConflictingServer {
                name: config.name.clone(),
            });
        }
        self.servers
            .insert(config.name.clone(), (config, availability));
        Ok(self)
    }

    /// Supply only inline child profiles. Validate all profiles before changing
    /// any so conflicting input cannot leave a partially supplied definition.
    pub(crate) fn supply(&self, definition: &mut MobDefinition) -> Result<(), MobError> {
        let offered: Vec<_> = self
            .servers
            .values()
            .filter(|(_, availability)| {
                *availability == ChildToolBundleAvailability::ChildAvailable
            })
            .map(|(config, _)| config)
            .collect();
        for binding in definition.profiles.values() {
            if let Some(profile) = binding.as_inline() {
                for config in &offered {
                    if profile
                        .tools
                        .mcp_servers
                        .iter()
                        .any(|existing| existing.name == config.name && existing != *config)
                    {
                        return Err(MobError::Internal(format!(
                            "conflicting child MCP server descriptor: {}",
                            config.name
                        )));
                    }
                }
            }
        }
        for binding in definition.profiles.values_mut() {
            if let Some(profile) = binding.as_inline_mut() {
                for config in &offered {
                    if !profile.tools.mcp_servers.contains(config) {
                        profile.tools.mcp_servers.push((*config).clone());
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "child_mcp_servers_tests.rs"]
mod tests;
