use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;

use crate::{ScheduleService, ScheduleToolSurface};

pub fn wire_schedule_tools(schedule_service: ScheduleService) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(ScheduleToolSurface::new(schedule_service))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    use crate::MemoryScheduleStore;

    #[test]
    fn wire_schedule_tools_exposes_builtin_schedule_tools() {
        let dispatcher = wire_schedule_tools(ScheduleService::new(Arc::new(
            MemoryScheduleStore::default(),
        )));
        let tool_names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();

        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_create"),
            "missing schedule create tool: {tool_names:?}"
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_list"),
            "missing schedule list tool: {tool_names:?}"
        );
    }
}
