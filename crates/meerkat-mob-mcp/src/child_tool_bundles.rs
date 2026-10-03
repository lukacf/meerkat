//! Host-registered Rust tool bundles for child mobs: mobs a member creates
//! with the agent `mob_create` tool, and the implicit mob `delegate` helpers
//! run in.
//!
//! The host registers each bundle with an availability, and the host alone
//! decides what child members get: every bundle it marks
//! [`ChildToolBundleAvailability::ChildAvailable`] is supplied to each inline
//! profile of a child mob when the mob is created. Callers never name
//! bundles; the public profile input has no `rust_bundles` field, but a
//! profile's deny list can still narrow what its members may call, since it
//! reads the resolved bundles. The supplied ids persist with the definition,
//! like the child application tool policy persists with its members, so a
//! bundle the host later withdraws is neither dropped nor substituted: the
//! member build refuses with `MobError::ToolBundleUnavailable` naming it.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;
use meerkat_mob::{MobBuilder, MobDefinition};

/// Whether the host supplies a bundle to child mob members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildToolBundleAvailability {
    /// Registered for the host only; never offered to child mobs.
    #[default]
    HostOnly,
    /// Supplied to every inline profile of a child mob.
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

    /// Supply every child-available bundle id to each inline profile of a
    /// child mob's definition. The ids persist with the definition, so a
    /// resumed member composes the same bundles. Realm-referenced profiles
    /// are host-stored and keep what the host named.
    pub(crate) fn supply(&self, definition: &mut MobDefinition) {
        let ids = self
            .bundles
            .iter()
            .filter(|(_, (_, availability))| {
                *availability == ChildToolBundleAvailability::ChildAvailable
            })
            .map(|(name, _)| name);
        let ids: Vec<&String> = ids.collect();
        if ids.is_empty() {
            return;
        }
        for binding in definition.profiles.values_mut() {
            if let Some(profile) = binding.as_inline_mut() {
                for id in &ids {
                    if !profile.tools.rust_bundles.contains(id) {
                        profile.tools.rust_bundles.push((*id).clone());
                    }
                }
            }
        }
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
