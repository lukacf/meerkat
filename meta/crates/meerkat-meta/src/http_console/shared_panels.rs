//! Read-only inspection of the native stores registered with a shared console.
//! Runtime selection is explicit when more than one repository is present.
use super::*;
use crate::console_aggregator::ConsolePanelRuntime;

#[derive(serde::Serialize)]
pub(super) struct PanelRuntimeDescriptor<'a> {
    key: &'a str,
    label: String,
    namespace: &'a str,
    memory: bool,
    workgraph: bool,
}

pub(super) fn descriptor(runtime: &ConsolePanelRuntime) -> PanelRuntimeDescriptor<'_> {
    PanelRuntimeDescriptor {
        key: &runtime.key,
        label: runtime.runtime.handle().mob_id().to_string(),
        namespace: &runtime.namespace,
        memory: runtime.memory.is_some(),
        workgraph: runtime.runtime.workgraph_service().is_some(),
    }
}

pub(super) const MEMORY_READ_METHODS: &[&str] = &[
    "meta/memory/panel/records",
    "meta/memory/panel/record",
    "meta/memory/panel/quarantine",
    "meta/memory/panel/dreams",
    "meta/memory/panel/dream_runs",
    "meta/memory/panel/audit_verdicts",
    "meta/memory/panel/overview",
    "meta/memory/panel/proposals",
    "meta/memory/panel/injections",
    "meta/memory/panel/harvests",
];

pub(super) fn is_read(method: &str) -> bool {
    MEMORY_READ_METHODS.contains(&method)
        || crate::rpc::workgraph_methods::is_workgraph_read_method(method)
}

pub(super) fn supports(runtime: &ConsolePanelRuntime, method: &str) -> bool {
    if MEMORY_READ_METHODS.contains(&method) {
        runtime.memory.is_some()
    } else {
        runtime.runtime.workgraph_service().is_some()
    }
}

pub(super) async fn dispatch(
    aggregator: &MetaConsoleAggregator,
    request: &JsonRpcRequest,
    access_view: Option<&AccessView>,
) -> Value {
    let id = request.id.clone().unwrap_or(Value::Null);
    let mut params = request.params.clone();
    let Some(object) = params.as_object_mut() else {
        return invalid_params(id, "panel params must be an object");
    };
    let requested = match object.remove("runtime_key") {
        Some(Value::String(key)) if !key.is_empty() => Some(key),
        None => None,
        _ => return invalid_params(id, "runtime_key must be a nonempty string"),
    };
    let runtimes = match aggregator.panel_runtimes() {
        Ok(runtimes) => runtimes,
        Err(error) => return internal_error(id, error.to_string()),
    };
    let selected = match requested.as_deref() {
        Some(key) => runtimes.iter().find(|runtime| runtime.key == key),
        None if runtimes.len() == 1 => runtimes.first(),
        None => return invalid_params(id, "Choose a repository runtime using runtime_key"),
    };
    let Some(selected) = selected else {
        return invalid_params(
            id,
            "The selected repository runtime is no longer registered",
        );
    };
    let store = selected.memory.as_deref();
    let stale_id = id.clone();
    let response = match request.method.as_str() {
        "meta/memory/panel/records" => {
            handle_memory_panel_records(store, access_view, &params, id).await
        }
        "meta/memory/panel/record" => {
            handle_memory_panel_record(store, access_view, &params, id).await
        }
        "meta/memory/panel/quarantine" => {
            handle_memory_panel_quarantine(store, access_view, &params, id).await
        }
        "meta/memory/panel/dreams" => handle_memory_panel_dreams(store, &params, id).await,
        "meta/memory/panel/dream_runs" => handle_memory_panel_dream_runs(store, &params, id).await,
        "meta/memory/panel/audit_verdicts" => {
            handle_memory_panel_audit_verdicts(store, &params, id).await
        }
        "meta/memory/panel/overview" => handle_memory_panel_overview(store, &params, id).await,
        "meta/memory/panel/proposals" => handle_memory_panel_proposals(store, &params, id).await,
        "meta/memory/panel/injections" => handle_memory_panel_injections(store, &params, id).await,
        "meta/memory/panel/harvests" => handle_memory_panel_harvests(store, &params, id).await,
        method => {
            let admission = selected.runtime.workgraph_admission();
            let service = selected.runtime.workgraph_service();
            match crate::rpc::workgraph_methods::handle_workgraph_method(
                service.as_ref(),
                &admission,
                crate::rpc::workgraph_methods::WorkgraphSurface::Console {
                    authenticated_principal: None,
                },
                method,
                &params,
            )
            .await
            {
                Ok(result) => response_value(id, Some(result), None),
                Err(error) => response_value(id, None, Some(error)),
            }
        }
    };
    if aggregator.panel_runtime_is_current(selected) {
        response
    } else {
        invalid_params(
            stale_id,
            "The selected repository runtime changed during the read",
        )
    }
}
