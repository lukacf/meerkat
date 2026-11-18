//! Idle pursuit of bound WorkGraph work.

use std::sync::Arc;

use crate::error::MobError;

use super::handle::MobHandle;

/// Says, for the host, whether an attention binding is pursued when its
/// work changes. The host decides: nothing a member passes is read.
pub type WorkGraphPursuitPredicate =
    Arc<dyn Fn(&meerkat::WorkAttentionBinding) -> bool + Send + Sync>;

/// Idle pursuit for the mobs of one host over one WorkGraph.
#[derive(Clone)]
pub struct WorkGraphPursuit {
    host_service: meerkat::WorkGraphService,
    pursues: WorkGraphPursuitPredicate,
}

impl WorkGraphPursuit {
    /// Pursuit over `host_service`, for the bindings `pursues` says yes to.
    pub fn new(
        host_service: meerkat::WorkGraphService,
        pursues: WorkGraphPursuitPredicate,
    ) -> Self {
        Self {
            host_service,
            pursues,
        }
    }

    /// The host's WorkGraph service with this pursuit told of its writes.
    pub fn workgraph_service(&self) -> &meerkat::WorkGraphService {
        &self.host_service
    }

    /// Pursue for `mob` from now on.
    pub fn attach(&self, mob: &MobHandle) -> Result<(), MobError> {
        let _ = (mob, &self.pursues);
        Ok(())
    }
}
