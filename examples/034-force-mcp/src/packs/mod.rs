pub mod advisor;
pub mod architect;
pub mod brainstorm;
pub mod rct;
pub mod red_team;
pub mod review;

use std::collections::BTreeMap;

use meerkat_mob::definition::MobDefinition;
use serde_json::{Value, json};

/// JSON schema requiring `{"response": "..."}` — ensures flow engine can parse output.
pub fn text_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "response": { "type": "string", "description": "Your full response text" }
        },
        "required": ["response"],
        "additionalProperties": false
    })
}

/// A named pack that builds a MobDefinition for a specific collaboration pattern.
pub trait Pack: Send + Sync {
    /// Stable name used in the `deliberate` tool's `pack` parameter.
    fn name(&self) -> &str;
    /// Human-readable description.
    fn description(&self) -> &str;
    /// Number of agents this pack spawns.
    fn agent_count(&self) -> usize;
    /// Number of flow steps (for progress tracking total).
    fn flow_step_count(&self) -> usize;
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
        packs.insert("advisor".into(), Box::new(advisor::AdvisorPack));
        packs.insert("review".into(), Box::new(review::ReviewPack));
        packs.insert("architect".into(), Box::new(architect::ArchitectPack));
        packs.insert("brainstorm".into(), Box::new(brainstorm::BrainstormPack));
        packs.insert("red-team".into(), Box::new(red_team::RedTeamPack));
        packs.insert("rct".into(), Box::new(rct::RctPack));
        Self { packs }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Pack> {
        self.packs.get(name).map(|p| p.as_ref())
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.packs.keys().map(|s| s.as_str()).collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Pack> {
        self.packs.values().map(|p| p.as_ref())
    }
}
