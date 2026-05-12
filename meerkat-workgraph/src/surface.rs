use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;

use crate::{WorkGraphService, WorkGraphToolSurface};

pub fn wire_workgraph_tools(service: WorkGraphService) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(WorkGraphToolSurface::new(service))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryWorkGraphStore, WorkGraphService};

    #[test]
    fn wire_workgraph_tools_exposes_workgraph_tools() {
        let dispatcher =
            wire_workgraph_tools(WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new())));
        let names = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "workgraph_create"));
        assert!(names.iter().any(|name| name == "workgraph_ready"));
    }
}
