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
    let catalog_options = if cfg!(feature = "mini-surface") {
        meerkat_contracts::RpcMethodCatalogOptions::mini_surface()
    } else {
        meerkat_contracts::RpcMethodCatalogOptions {
            runtime_available,
            mob_enabled: cfg!(feature = "mob"),
            mcp_enabled: cfg!(feature = "mcp"),
            comms_enabled: cfg!(feature = "comms"),
            blob_enabled: true,
            session_events_enabled: true,
            session_streams_enabled: true,
            schedule_enabled: cfg!(feature = "schedule"),
            skills_enabled: true,
        }
    };
    let mut options = meerkat::surface::RuntimeHostSurfaceOptions::process(
        "meerkat-rpc",
        env!("CARGO_PKG_VERSION"),
    );
    options.runtime_backed_sessions = runtime_available;
    options.mobs = cfg!(feature = "mob");
    options.mcp_live = cfg!(feature = "mcp");
    options.comms = cfg!(feature = "comms");
    options.blobs = true;
    options.artifacts = true;
    options.session_events = true;
    options.event_replay = event_replay;
    options.session_streams = true;
    options.schedules = cfg!(feature = "schedule");
    options.skills = true;
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
