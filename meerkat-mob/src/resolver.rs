use crate::error::MobResult;
use crate::model::{MeerkatIdentity, MobSpec, ResolverSpec};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct ResolverContext {
    pub mob_id: String,
    pub role: String,
    pub resolver_id: String,
    pub resolver_spec: Option<ResolverSpec>,
    pub spec: Option<MobSpec>,
    pub activation_payload: Value,
}

#[async_trait]
pub trait MeerkatResolver: Send + Sync {
    async fn list_meerkats(&self, context: &ResolverContext) -> MobResult<Vec<MeerkatIdentity>>;
}

#[derive(Default, Clone)]
pub struct ResolverRegistry {
    resolvers: BTreeMap<String, Arc<dyn MeerkatResolver>>,
}

impl ResolverRegistry {
    pub fn register(&mut self, id: impl Into<String>, resolver: Arc<dyn MeerkatResolver>) {
        self.resolvers.insert(id.into(), resolver);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn MeerkatResolver>> {
        self.resolvers.get(id).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.resolvers.contains_key(id)
    }
}
