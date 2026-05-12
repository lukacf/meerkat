//! Handlers for `runtime/*` host projection methods.

use std::sync::Arc;

use meerkat_core::ConfigStore;

use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

fn host_surface_options(
    runtime_available: bool,
    event_replay: bool,
    approvals_available: bool,
) -> meerkat::surface::RuntimeHostSurfaceOptions {
    let catalog_options = super::initialize::rpc_method_catalog_options(runtime_available);
    let mut options = meerkat::surface::RuntimeHostSurfaceOptions::process(
        "meerkat-rpc",
        env!("CARGO_PKG_VERSION"),
    );
    options.runtime_backed_sessions = catalog_options.runtime_available;
    options.mobs = catalog_options.mob_enabled;
    options.mcp_live = catalog_options.mcp_enabled;
    options.comms = catalog_options.comms_enabled;
    options.blobs = catalog_options.blob_enabled;
    options.artifacts = catalog_options.blob_enabled;
    options.session_events = catalog_options.session_events_enabled;
    options.event_replay = catalog_options.session_events_enabled && event_replay;
    options.session_streams = catalog_options.session_streams_enabled;
    options.schedules = catalog_options.schedule_enabled;
    options.skills = catalog_options.skills_enabled;
    options.approvals = approvals_available;
    options.rpc_transport = Some("json_rpc".to_string());
    options.rpc_methods = meerkat_contracts::rpc_method_names(catalog_options);
    options
}

pub fn handle_info(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    config_store: &Arc<dyn ConfigStore>,
    runtime_available: bool,
) -> RpcResponse {
    let options = host_surface_options(
        runtime_available,
        runtime.supports_event_replay(),
        runtime.approval_service().is_persistent(),
    );
    let (context_root, _) = runtime.skill_identity_roots();
    let metadata = config_store.metadata();
    let metadata = metadata
        .as_ref()
        .map(meerkat::surface::RuntimeHostMetadataProjection::from);
    let info = meerkat::surface::build_runtime_host_info(&options, metadata.as_ref(), context_root);
    RpcResponse::success(id, &info)
}

pub fn handle_capabilities(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    runtime_available: bool,
) -> RpcResponse {
    let options = host_surface_options(
        runtime_available,
        runtime.supports_event_replay(),
        runtime.approval_service().is_persistent(),
    );
    let capabilities = meerkat::surface::build_runtime_host_capabilities(&options);
    RpcResponse::success(id, &capabilities)
}

pub fn handle_health(id: Option<RpcId>) -> RpcResponse {
    let health = meerkat::surface::build_runtime_host_health();
    RpcResponse::success(id, &health)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_surface_capabilities_are_catalog_projection() {
        let runtime_available = true;
        let event_replay = true;
        let approvals_available = true;
        let catalog_options =
            super::super::initialize::rpc_method_catalog_options(runtime_available);
        let options = host_surface_options(runtime_available, event_replay, approvals_available);

        assert_eq!(
            options.runtime_backed_sessions,
            catalog_options.runtime_available
        );
        assert_eq!(options.mobs, catalog_options.mob_enabled);
        assert_eq!(options.mcp_live, catalog_options.mcp_enabled);
        assert_eq!(options.comms, catalog_options.comms_enabled);
        assert_eq!(options.blobs, catalog_options.blob_enabled);
        assert_eq!(options.artifacts, catalog_options.blob_enabled);
        assert_eq!(
            options.session_events,
            catalog_options.session_events_enabled
        );
        assert_eq!(
            options.event_replay,
            catalog_options.session_events_enabled && event_replay
        );
        assert_eq!(
            options.session_streams,
            catalog_options.session_streams_enabled
        );
        assert_eq!(options.schedules, catalog_options.schedule_enabled);
        assert_eq!(options.skills, catalog_options.skills_enabled);
        assert_eq!(
            options.rpc_methods,
            meerkat_contracts::rpc_method_names(catalog_options)
        );
    }
}
