#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, DetachedToolExecutionPolicy, DynamicToolComposite, IdempotencyScope,
    RestartClass, RunnerIdentity, SessionId, ToolCatalogCapabilities, ToolCatalogEntry, ToolError,
    ToolExecutionContract, ToolExecutionMode,
    types::{ToolCallView, ToolDef},
};
use meerkat_schedule::{
    CurrentSessionScheduleToolDispatcher, MemoryScheduleStore, ScheduleService,
    ScheduleToolDispatcher, schedule_tools_list,
};
use serde_json::{json, value::RawValue};

struct ExactSibling {
    entry: ToolCatalogEntry,
}

#[async_trait]
impl AgentToolDispatcher for ExactSibling {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::clone(&self.entry.tool)])
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::from([self.entry.clone()])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

fn schedules() -> ScheduleToolDispatcher {
    ScheduleToolDispatcher::new(ScheduleService::new(Arc::new(MemoryScheduleStore::new())))
}

#[tokio::test]
async fn static_schedule_catalog_is_complete_exact_and_fast_only() {
    let dispatcher = schedules();
    let capabilities = dispatcher.tool_catalog_capabilities();
    assert!(capabilities.exact_catalog);
    assert!(!capabilities.may_require_catalog_control_plane);
    let tools = dispatcher.tools();
    let catalog = dispatcher.tool_catalog();
    assert_eq!(catalog.len(), schedule_tools_list().len());
    assert_eq!(tools.len(), catalog.len());
    let names = catalog
        .iter()
        .map(|entry| &entry.tool.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), catalog.len());
    for (tool, entry) in tools.iter().zip(catalog.iter()) {
        assert_eq!(
            serde_json::to_value(tool).unwrap(),
            serde_json::to_value(&entry.tool).unwrap()
        );
        assert!(entry.callability.is_callable());
        assert_eq!(
            entry.execution.supported_modes(),
            &BTreeSet::from([ToolExecutionMode::Fast])
        );
    }
    let args = RawValue::from_string("{}".into()).unwrap();
    let result = dispatcher
        .dispatch(ToolCallView {
            id: "unknown",
            name: "not_a_schedule_tool",
            args: &args,
        })
        .await;
    assert!(matches!(result, Err(ToolError::NotFound { .. })));
}

#[test]
fn nested_schedule_and_current_session_catalogs_preserve_complete_sibling_metadata() {
    let expected = ToolCatalogEntry::session_inline(
        Arc::new(ToolDef::new(
            "synthetic_detached",
            "No-effect declaration",
            json!({"type":"object"}),
        )),
        true,
    )
    .with_execution_contract(
        ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Fast, ToolExecutionMode::Detached]),
            ToolExecutionMode::Fast,
            None,
            Some(
                DetachedToolExecutionPolicy::new(
                    RunnerIdentity::new("synthetic.detached", "v1").unwrap(),
                    RestartClass::NonResumable,
                    IdempotencyScope::ToolCall,
                    Duration::from_secs(10),
                )
                .unwrap(),
            ),
        )
        .unwrap(),
    );
    for current_session in [false, true] {
        let schedule: Arc<dyn AgentToolDispatcher> = Arc::new(schedules());
        let schedule: Arc<dyn AgentToolDispatcher> = if current_session {
            Arc::new(CurrentSessionScheduleToolDispatcher::new(
                schedule,
                SessionId::new(),
            ))
        } else {
            schedule
        };
        let mut dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(DynamicToolComposite::new(vec![
                Arc::new(ExactSibling {
                    entry: expected.clone(),
                }),
                schedule,
            ]));
        for _ in 0..3 {
            assert!(dispatcher.tool_catalog_capabilities().exact_catalog);
            let catalog = dispatcher.tool_catalog();
            let entry = catalog
                .iter()
                .find(|entry| entry.tool.name == expected.tool.name)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&entry.tool).unwrap(),
                serde_json::to_value(&expected.tool).unwrap()
            );
            assert_eq!(entry.execution, expected.execution);
            assert_eq!(entry.plane, expected.plane);
            assert_eq!(entry.callability, expected.callability);
            assert_eq!(entry.deferred_eligibility, expected.deferred_eligibility);
            dispatcher = Arc::new(DynamicToolComposite::new(vec![dispatcher]));
        }
    }
}
