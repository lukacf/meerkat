//! Shared surface infrastructure helpers.
//!
//! Cross-cutting helpers used by all protocol surfaces (RPC, REST, MCP Server).

use meerkat_contracts::{
    CapabilitiesResponse, CapabilityEntry, CapabilityStatus, ContractVersion, build_capabilities,
};
use meerkat_core::{AgentEvent, Config};
use tokio::sync::mpsc;

#[cfg(feature = "skills")]
use meerkat_core::skills::{
    SkillDocument, SkillError, SkillFilter, SkillId, SkillIntrospectionEntry,
    SkillRuntime,
};
#[cfg(feature = "skills")]
use std::sync::Arc;

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
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            callback(event);
        }
    });
    tx
}
