//! Shared surface infrastructure helpers.
//!
//! Cross-cutting helpers used by all protocol surfaces (RPC, REST, MCP Server).

mod request_execution;
mod stdio_json;

pub use request_execution::{
    PreparedSurfaceSession, RequestAsyncAction, RequestContext, RequestTerminal,
    SurfaceRequestExecutor, noop_request_action, prepare_surface_session, request_action,
};
pub use stdio_json::{StdioJsonWriter, spawn_stdio_json_writer};

use meerkat_contracts::{
    CapabilitiesResponse, CapabilityEntry, CapabilityStatus, ContractVersion, build_capabilities,
};
use meerkat_core::{AgentEvent, Config, PeerMeta};

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "skills")]
use meerkat_core::skills::{
    SkillDocument, SkillError, SkillFilter, SkillId, SkillIntrospectionEntry, SkillRuntime,
};
#[cfg(feature = "mcp")]
use std::collections::HashMap;
#[cfg(feature = "mcp")]
use std::collections::VecDeque;
#[cfg(feature = "skills")]
use std::sync::Arc;
#[cfg(feature = "mcp")]
use std::sync::{Mutex, OnceLock};

/// Build a [`CapabilitiesResponse`] with status resolved against config.
///
/// For each registered capability, calls its `status_resolver` (if provided)
/// to determine runtime status. Capabilities without a resolver are reported
/// as `Available`. This keeps policy knowledge in the owning crate.
pub fn build_capabilities_response(config: &Config) -> CapabilitiesResponse {
    let registrations = build_capabilities();
    let capabilities = registrations
        .into_iter()
        .map(|reg| {
            let status = match reg.status_resolver {
                Some(resolver) => resolver(config),
                None => CapabilityStatus::Available,
            };
            CapabilityEntry {
                id: reg.id,
                description: reg.description.to_string(),
                status,
            }
        })
        .collect();

    CapabilitiesResponse {
        contract_version: ContractVersion::CURRENT,
        capabilities,
    }
}

/// Build a [`ModelsCatalogResponse`] from the compiled-in catalog.
///
/// This is a pure function with no config dependency — the catalog is static data
/// compiled into the binary from `meerkat-models`.
pub fn build_models_catalog_response() -> meerkat_contracts::ModelsCatalogResponse {
    let providers = meerkat_models::provider_defaults()
        .iter()
        .map(|pd| {
            let models = pd
                .models
                .iter()
                .map(|entry| {
                    let profile = meerkat_models::profile_for(entry.provider, entry.id).map(|p| {
                        meerkat_contracts::WireModelProfile {
                            model_family: p.model_family,
                            supports_temperature: p.supports_temperature,
                            supports_thinking: p.supports_thinking,
                            supports_reasoning: p.supports_reasoning,
                            params_schema: p.params_schema,
                        }
                    });
                    meerkat_contracts::CatalogModelEntry {
                        id: entry.id.to_string(),
                        display_name: entry.display_name.to_string(),
                        tier: match entry.tier {
                            meerkat_models::ModelTier::Recommended => {
                                meerkat_contracts::WireModelTier::Recommended
                            }
                            meerkat_models::ModelTier::Supported => {
                                meerkat_contracts::WireModelTier::Supported
                            }
                        },
                        context_window: entry.context_window,
                        max_output_tokens: entry.max_output_tokens,
                        profile,
                    }
                })
                .collect();
            meerkat_contracts::ProviderCatalog {
                provider: pd.provider.to_string(),
                default_model_id: pd.default_model_id.to_string(),
                models,
            }
        })
        .collect();

    meerkat_contracts::ModelsCatalogResponse {
        contract_version: ContractVersion::CURRENT,
        providers,
    }
}

/// Validate whether host mode can be enabled in the current build.
///
/// Delegates to `meerkat_comms::validate_host_mode()` when the comms feature
/// is compiled in; returns an error if requested but comms is not available.
///
/// This is the canonical entry point — all surfaces should call this.
pub fn resolve_host_mode(requested: bool) -> Result<bool, String> {
    #[cfg(feature = "comms")]
    {
        meerkat_comms::validate_host_mode(requested)
    }
    #[cfg(not(feature = "comms"))]
    {
        if requested {
            return Err(
                "host_mode requires comms support (build with --features comms)".to_string(),
            );
        }
        Ok(false)
    }
}

const RESERVED_MOB_PEER_META_LABELS: [&str; 3] = ["mob_id", "role", "meerkat_id"];

/// Reject public `PeerMeta` labels that are reserved for mob-managed sessions.
///
/// Mob runtime code stamps these labels internally when provisioning members.
/// Public protocol surfaces must reject caller-supplied values so ordinary
/// sessions cannot spoof durable mob ownership markers.
pub fn validate_public_peer_meta(peer_meta: Option<&PeerMeta>) -> Result<(), String> {
    let Some(peer_meta) = peer_meta else {
        return Ok(());
    };

    for label in RESERVED_MOB_PEER_META_LABELS {
        if peer_meta.labels.contains_key(label) {
            return Err(format!(
                "peer_meta label '{label}' is reserved for mob-managed sessions"
            ));
        }
    }

    Ok(())
}

/// List all skills with provenance and shadow information.
#[cfg(feature = "skills")]
///
/// Returns `None` if the skill runtime is not available.
pub async fn list_skills_introspection(
    skill_runtime: &Option<Arc<SkillRuntime>>,
    filter: &SkillFilter,
) -> Option<Result<Vec<SkillIntrospectionEntry>, SkillError>> {
    let runtime = skill_runtime.as_ref()?;
    Some(runtime.list_all_with_provenance(filter).await)
}

/// Load and inspect a skill by ID, optionally from a specific source.
#[cfg(feature = "skills")]
///
/// Returns `None` if the skill runtime is not available.
pub async fn inspect_skill(
    skill_runtime: &Option<Arc<SkillRuntime>>,
    id: &SkillId,
    source_name: Option<&str>,
) -> Option<Result<SkillDocument, SkillError>> {
    let runtime = skill_runtime.as_ref()?;
    Some(runtime.load_from_source(id, source_name).await)
}

/// Spawn a task that forwards agent events from a channel to a callback.
///
/// Returns the sender half of the channel. The spawned task runs until
/// the sender is dropped.
pub fn spawn_event_forwarder<F>(callback: F) -> mpsc::Sender<AgentEvent>
where
    F: Fn(AgentEvent) + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    #[cfg(not(target_arch = "wasm32"))]
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            callback(event);
        }
    });
    #[cfg(target_arch = "wasm32")]
    tokio_with_wasm::alias::task::spawn(async move {
        while let Some(event) = rx.recv().await {
            callback(event);
        }
    });

    tx
}

// ---------------------------------------------------------------------------
// Live MCP surface helpers (shared across RPC, REST)
// ---------------------------------------------------------------------------

/// Build a canonical [`McpLiveOpResponse`] for a staged operation.
///
/// All surfaces use this to ensure identical response shaping — operation,
/// server name, persisted handling, and `applied_at_turn` semantics.
#[cfg(feature = "mcp")]
pub fn mcp_live_response(
    session_id: String,
    operation: meerkat_contracts::McpLiveOperation,
    server_name: Option<String>,
) -> meerkat_contracts::McpLiveOpResponse {
    meerkat_contracts::McpLiveOpResponse {
        session_id,
        operation,
        server_name,
        status: meerkat_contracts::McpLiveOpStatus::Staged,
        persisted: false,
        applied_at_turn: None,
    }
}

/// Resolve the `persisted` flag from a caller request.
///
/// Config persistence is not yet implemented — if the caller sends
/// `persisted=true` we log a warning and always return `false`.
/// All surfaces call this to avoid semantic drift.
#[cfg(feature = "mcp")]
pub fn resolve_persisted(operation: &str, persisted: bool) -> bool {
    if persisted {
        tracing::warn!(
            operation,
            "caller sent persisted=true but config persistence is not yet implemented; \
             responding with persisted=false"
        );
    }
    false
}

/// Validate that a reload target exists in the adapter's active server set.
///
/// Returns `Ok(())` if the server is active, or an error message suitable
/// for returning to the caller.
#[cfg(feature = "mcp")]
pub async fn validate_reload_target(
    adapter: &meerkat_mcp::McpRouterAdapter,
    server_name: &str,
) -> Result<(), String> {
    let active = adapter.active_server_names().await;
    if active.iter().any(|n| n == server_name) {
        Ok(())
    } else {
        Err(format!(
            "MCP server '{server_name}' is not registered on this session"
        ))
    }
}

/// Emit [`AgentEvent::ToolConfigChanged`] events for a batch of lifecycle actions.
///
/// Also appends `[system-notice]` lines to `prompt` for forced removals.
/// Both RPC and REST call this at turn boundaries.
#[cfg(feature = "mcp")]
pub async fn emit_mcp_lifecycle_events(
    event_tx: &mpsc::Sender<meerkat_core::EventEnvelope<AgentEvent>>,
    source_id: &str,
    prompt: &mut String,
    turn_number: u32,
    actions: Vec<meerkat_mcp::McpLifecycleAction>,
) {
    use meerkat_mcp::McpLifecyclePhase;

    const MCP_SEQ_SOURCE_CAP: usize = 8192;

    #[derive(Default)]
    struct McpSeqState {
        seq_by_source: HashMap<String, u64>,
        source_order: VecDeque<String>,
    }

    static MCP_EVENT_SEQ_BY_SOURCE: OnceLock<Mutex<McpSeqState>> = OnceLock::new();

    for action in actions {
        let mut payload = action.to_tool_config_changed_payload();
        payload.applied_at_turn = Some(turn_number);
        let target = payload.target.clone();
        let seq = {
            let map = MCP_EVENT_SEQ_BY_SOURCE.get_or_init(|| Mutex::new(McpSeqState::default()));
            let mut guard = match map.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if !guard.seq_by_source.contains_key(source_id) {
                let source_key = source_id.to_string();
                guard.source_order.push_back(source_key.clone());
                guard.seq_by_source.insert(source_key, 0);

                while guard.seq_by_source.len() > MCP_SEQ_SOURCE_CAP {
                    if let Some(evicted) = guard.source_order.pop_front() {
                        guard.seq_by_source.remove(&evicted);
                    } else {
                        break;
                    }
                }
            }

            let entry = guard
                .seq_by_source
                .entry(source_id.to_string())
                .or_insert(0);
            *entry += 1;
            *entry
        };
        let _ = event_tx
            .send(meerkat_core::EventEnvelope::new(
                source_id,
                seq,
                None,
                AgentEvent::ToolConfigChanged {
                    payload: payload.clone(),
                },
            ))
            .await;
        if action.phase == McpLifecyclePhase::Forced {
            if !prompt.is_empty() {
                prompt.push('\n');
            }
            prompt.push_str(&format!(
                "[system-notice] MCP server '{target}' removal forced after drain timeout."
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_public_peer_meta_rejects_reserved_labels() {
        let peer_meta = PeerMeta::default().with_label("mob_id", "team");
        let result = validate_public_peer_meta(Some(&peer_meta));
        assert!(
            result.is_err(),
            "reserved mob labels must be rejected on public surfaces"
        );
        let Err(err) = result else {
            unreachable!("asserted reserved labels are rejected above");
        };
        assert!(err.contains("mob-managed sessions"));
        assert!(err.contains("mob_id"));
    }

    #[test]
    fn validate_public_peer_meta_allows_unreserved_labels() {
        let peer_meta = PeerMeta::default().with_label("team", "infra");
        let result = validate_public_peer_meta(Some(&peer_meta));
        assert!(
            result.is_ok(),
            "ordinary peer metadata should stay available"
        );
    }
}
