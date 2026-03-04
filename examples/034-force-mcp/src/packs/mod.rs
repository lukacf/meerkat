use std::collections::BTreeMap;

use meerkat_mob::definition::MobDefinition;

/// A named pack that builds a MobDefinition for a specific collaboration pattern.
pub trait Pack: Send + Sync {
    /// Stable name used in the `deliberate` tool's `pack` parameter.
    fn name(&self) -> &str;
    /// Human-readable description.
    fn description(&self) -> &str;
    /// Number of agents this pack spawns.
    fn agent_count(&self) -> usize;
    /// Build the MobDefinition with task/context interpolated and model overrides applied.
    fn definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
    ) -> MobDefinition;
}

/// Registry of all available packs.
pub struct PackRegistry {
    packs: BTreeMap<String, Box<dyn Pack>>,
}

impl PackRegistry {
    pub fn new() -> Self {
        let mut packs = BTreeMap::<String, Box<dyn Pack>>::new();

        // Packs will be registered here as they're implemented
        // packs.insert("advisor".into(), Box::new(advisor::AdvisorPack));
        // packs.insert("review".into(), Box::new(review::ReviewPack));
        // ...

        Self { packs }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Pack> {
        self.packs.get(name).map(|p| p.as_ref())
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.packs.keys().map(|s| s.as_str()).collect()
    }
}
