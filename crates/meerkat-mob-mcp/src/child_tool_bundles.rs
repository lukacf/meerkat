//! Host-registered Rust tool bundles for mobs that callers of the mob tools
//! create (agent `mob_create` and public `meerkat_mob_create`).
//!
//! The host registers each bundle with an availability. A caller's profile
//! may name only bundles the host marked [`ChildToolBundleAvailability::ChildAvailable`];
//! it supplies ids, never implementations. Child mob builders receive only
//! those bundles, so a bundle the host later unregisters or marks host-only
//! fails the member build through meerkat-mob's missing-bundle refusal.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;
use meerkat_mob::{MobBuilder, MobDefinition};

/// Whether a host bundle may be named by child mob profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildToolBundleAvailability {
    /// Registered for the host only; never offered to child mobs.
    #[default]
    HostOnly,
    /// Child mob profiles may name it.
    ChildAvailable,
}

/// The host's bundle registrations as seen by child mob creation.
#[derive(Clone, Default)]
pub struct ChildToolBundles {
    bundles: BTreeMap<String, (Arc<dyn AgentToolDispatcher>, ChildToolBundleAvailability)>,
}

impl std::fmt::Debug for ChildToolBundles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_map()
            .entries(
                self.bundles
                    .iter()
                    .map(|(name, (_, availability))| (name, availability)),
            )
            .finish()
    }
}

/// Why a caller's child profile was refused. Whether the bundle is missing or
/// host-only is not disclosed to the caller: both read as not available.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tool bundle '{bundle}' named by profile '{profile}' is not available to child mobs")]
pub struct ChildToolBundleRefused {
    pub profile: String,
    pub bundle: String,
}

impl ChildToolBundles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `dispatcher` under `name`; a later registration replaces it.
    #[must_use]
    pub fn register(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn AgentToolDispatcher>,
        availability: ChildToolBundleAvailability,
    ) -> Self {
        self.bundles.insert(name.into(), (dispatcher, availability));
        self
    }

    /// Refuse a caller-supplied definition naming any bundle that is not
    /// registered as child-available.
    pub fn admit(&self, definition: &MobDefinition) -> Result<(), ChildToolBundleRefused> {
        for (profile, binding) in &definition.profiles {
            // Realm references resolve to host-stored profiles; the caller
            // cannot write bundle ids into those (decoding empties them).
            let Some(profile_definition) = binding.as_inline() else {
                continue;
            };
            for bundle in &profile_definition.tools.rust_bundles {
                if !matches!(
                    self.bundles.get(bundle),
                    Some((_, ChildToolBundleAvailability::ChildAvailable))
                ) {
                    return Err(ChildToolBundleRefused {
                        profile: profile.to_string(),
                        bundle: bundle.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Register the child-available bundles on a child mob builder.
    pub(crate) fn configure(&self, mut builder: MobBuilder) -> MobBuilder {
        for (name, (dispatcher, availability)) in &self.bundles {
            if *availability == ChildToolBundleAvailability::ChildAvailable {
                builder = builder.register_tool_bundle(name.clone(), Arc::clone(dispatcher));
            }
        }
        builder
    }
}
