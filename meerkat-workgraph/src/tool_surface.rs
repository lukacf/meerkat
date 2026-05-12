use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use serde_json::Value;

use crate::{WorkGraphService, handle_workgraph_tools_call, workgraph_tools_list};

pub struct WorkGraphToolSurface {
    service: WorkGraphService,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl WorkGraphToolSurface {
    pub fn new(service: WorkGraphService) -> Self {
        Self {
            service,
            tool_defs: build_tool_defs(),
        }
    }

    pub fn service(&self) -> &WorkGraphService {
        &self.service
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for WorkGraphToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if !self.tool_defs.iter().any(|tool| tool.name == call.name) {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result = handle_workgraph_tools_call(&self.service, call.name, &args)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                message: format!("{} (code {})", error.message, error.code),
            })?;
        Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
    }
}

fn build_tool_defs() -> Arc<[Arc<ToolDef>]> {
    workgraph_tools_list()
        .into_iter()
        .map(|tool| {
            Arc::new(ToolDef {
                name: tool["name"].as_str().unwrap_or_default().into(),
                description: tool["description"].as_str().unwrap_or_default().to_string(),
                input_schema: tool["inputSchema"].clone(),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::WorkGraph,
                    source_id: "workgraph".into(),
                }),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::{MemoryWorkGraphStore, WorkGraphService};

    #[tokio::test]
    async fn workgraph_tool_surface_dispatches_tools() {
        let surface =
            WorkGraphToolSurface::new(WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new())));
        let args = serde_json::value::RawValue::from_string(
            json!({ "title": "surface item" }).to_string(),
        )
        .unwrap();
        let outcome = surface
            .dispatch(ToolCallView {
                id: "call-1",
                name: "workgraph_create",
                args: &args,
            })
            .await
            .expect("dispatch");
        let value: Value = serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(value["item"]["title"].as_str(), Some("surface item"));
    }

    #[test]
    fn workgraph_tool_defs_have_workgraph_provenance() {
        let surface =
            WorkGraphToolSurface::new(WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new())));
        assert!(surface.tools().iter().all(|tool| {
            tool.provenance
                .as_ref()
                .is_some_and(|p| p.kind == ToolSourceKind::WorkGraph)
        }));
    }
}
