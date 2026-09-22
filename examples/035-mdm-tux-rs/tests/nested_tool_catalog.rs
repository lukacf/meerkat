use std::{collections::BTreeSet, sync::Arc, time::Duration};

use meerkat_core::{
    AgentToolDispatcher, DetachedToolExecutionPolicy, DynamicToolComposite, IdempotencyScope,
    RestartClass, RunnerIdentity, ToolCatalogCapabilities, ToolCatalogEntry, ToolExecutionContract,
    ToolExecutionMode,
    types::{ToolCallView, ToolDef},
};

struct DetachedCatalogFixture {
    entry: ToolCatalogEntry,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for DetachedCatalogFixture {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([self.entry.tool.clone()])
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
        _call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        unreachable!("catalog inspection must not execute the fixture")
    }
}

#[test]
fn nested_schedule_composition_preserves_sibling_execution_contract() {
    let contract = ToolExecutionContract::new(
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
    .unwrap();
    let fixture = Arc::new(DetachedCatalogFixture {
        entry: ToolCatalogEntry::session_inline(
            Arc::new(ToolDef::new(
                "synthetic_detached",
                "Synthetic declaration only",
                serde_json::json!({"type": "object"}),
            )),
            true,
        )
        .with_execution_contract(contract.clone()),
    });
    let schedules = Arc::new(meerkat::ScheduleToolDispatcher::new(
        meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new())),
    ));
    let inner = Arc::new(DynamicToolComposite::new(vec![fixture, schedules]));
    let inner_entry = inner
        .tool_catalog()
        .iter()
        .find(|entry| entry.tool.name == "synthetic_detached")
        .unwrap()
        .clone();
    assert_eq!(inner_entry.execution, contract);

    // The factory adds another composition layer when mob tools are enabled.
    let outer = DynamicToolComposite::new(vec![inner]);
    let outer_catalog = outer.tool_catalog();
    let outer_entry = outer_catalog
        .iter()
        .find(|entry| entry.tool.name == "synthetic_detached")
        .unwrap();
    assert_eq!(outer_entry.execution, contract);
}
