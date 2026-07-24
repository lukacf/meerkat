//! Handlers for `runtime/*` host projection methods.

use std::sync::Arc;

use meerkat_core::ConfigStore;

use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

fn host_surface_options(
    runtime_available: bool,
    live_enabled: bool,
    live_webrtc_enabled: bool,
    event_replay: bool,
    approvals_available: bool,
    skills_enabled: bool,
) -> meerkat::surface::RuntimeHostSurfaceOptions {
    let catalog_options = meerkat_contracts::RpcMethodCatalogOptions {
        runtime_available,
        live_enabled,
        mob_enabled: cfg!(feature = "mob"),
        mcp_enabled: cfg!(feature = "mcp"),
        comms_enabled: cfg!(feature = "comms"),
        blob_enabled: true,
        session_events_enabled: true,
        session_streams_enabled: true,
        schedule_enabled: cfg!(feature = "schedule"),
        workgraph_enabled: cfg!(feature = "workgraph"),
        skills_enabled,
        live_webrtc_enabled,
    };
    let mut options = meerkat::surface::RuntimeHostSurfaceOptions::process(
        "meerkat-rpc",
        env!("CARGO_PKG_VERSION"),
    );
    options.runtime_backed_sessions = runtime_available;
    options.mobs = cfg!(feature = "mob");
    options.multi_host_mobs = cfg!(feature = "mob");
    options.mcp_live = cfg!(feature = "mcp");
    options.comms = cfg!(feature = "comms");
    options.blobs = true;
    options.artifacts = true;
    options.session_events = true;
    options.event_replay = event_replay;
    options.session_streams = true;
    options.schedules = cfg!(feature = "schedule");
    options.skills = skills_enabled;
    options.approvals = approvals_available;
    options.durable_jobs = true;
    options.rpc_transport = Some("json_rpc".to_string());
    options.rpc_methods = meerkat_contracts::rpc_method_names(catalog_options);
    options
}

pub async fn handle_info(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    config_store: &Arc<dyn ConfigStore>,
    runtime_available: bool,
    live_enabled: bool,
    live_webrtc_enabled: bool,
    skills_enabled: bool,
) -> RpcResponse {
    let options = host_surface_options(
        runtime_available,
        live_enabled,
        live_webrtc_enabled,
        runtime.supports_event_replay(),
        runtime.approval_service().is_persistent(),
        skills_enabled,
    );
    let (context_root, _) = runtime.skill_identity_roots();
    let metadata = config_store.metadata();
    let metadata = metadata
        .as_ref()
        .map(meerkat::surface::RuntimeHostMetadataProjection::from);
    let mut info =
        meerkat::surface::build_runtime_host_info(&options, metadata.as_ref(), context_root);
    info.health = runtime_health(runtime).await;
    RpcResponse::success(id, &info)
}

pub fn handle_capabilities(
    id: Option<RpcId>,
    runtime: &Arc<SessionRuntime>,
    runtime_available: bool,
    live_enabled: bool,
    live_webrtc_enabled: bool,
    skills_enabled: bool,
) -> RpcResponse {
    let options = host_surface_options(
        runtime_available,
        live_enabled,
        live_webrtc_enabled,
        runtime.supports_event_replay(),
        runtime.approval_service().is_persistent(),
        skills_enabled,
    );
    let capabilities = meerkat::surface::build_runtime_host_capabilities(&options);
    RpcResponse::success(id, &capabilities)
}

pub async fn handle_health(id: Option<RpcId>, runtime: &Arc<SessionRuntime>) -> RpcResponse {
    let health = runtime_health(runtime).await;
    RpcResponse::success(id, &health)
}

async fn runtime_health(runtime: &SessionRuntime) -> meerkat_contracts::RuntimeHostHealth {
    let mut health = meerkat::surface::build_runtime_host_health();
    let observed_at_ms = meerkat_core::time_compat::SystemTime::now()
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(u64::MAX);
    let service = runtime.detached_job_service();
    let job_snapshot = match runtime.realm_id() {
        Some(realm_id) => {
            service
                .health_snapshot_for_realm(realm_id.as_str(), observed_at_ms, 10_000)
                .await
        }
        None => service.health_snapshot(observed_at_ms, 10_000).await,
    };
    let jobs = match job_snapshot {
        Ok(snapshot)
            if !snapshot.is_degraded() && runtime.runtime_job_delivery_backlog().await == Ok(0) =>
        {
            meerkat_contracts::RuntimeHostHealthStatus::Ok
        }
        Ok(_) | Err(_) => meerkat_contracts::RuntimeHostHealthStatus::Degraded,
    };
    health.checks.insert("jobs".to_string(), jobs);
    if jobs != meerkat_contracts::RuntimeHostHealthStatus::Ok {
        health.status = meerkat_contracts::RuntimeHostHealthStatus::Degraded;
    }
    health
}
