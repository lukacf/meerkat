//! Capability exclusions for the canonical browser runtime composition.
//!
//! This profile narrows reachable capabilities without changing session
//! lifecycle, input admission, comms, or terminal classification.

pub use meerkat_core::runtime_profile::{
    RuntimeProfileCapability, RuntimeProfileClearingAction, RuntimeProfileId,
    RuntimeProfileRefusal, RuntimeProfileRefusalCode, RuntimeProfileRefusalData,
};

use crate::{CapabilityId, HostProcessCapabilityId, MobpackCapabilityId};

/// Browser composition capability authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BrowserRuntimeProfile;

impl BrowserRuntimeProfile {
    /// Require a capability on the canonical browser runtime path.
    pub fn require(
        self,
        capability: RuntimeProfileCapability,
    ) -> Result<(), RuntimeProfileRefusal> {
        use RuntimeProfileCapability as Capability;
        use RuntimeProfileClearingAction as Action;

        let (clearing_action, message) = match capability {
            Capability::InMemoryPersistence
            | Capability::ForegroundExecution
            | Capability::KeepAlive
            | Capability::Comms
            | Capability::TransientTurnContext
            | Capability::EmbeddedSkills => return Ok(()),
            Capability::DurablePersistence => (
                Action::UsePersistentRuntime,
                "the browser profile provides in-memory persistence; use a persistent runtime for storage beyond the page lifetime",
            ),
            Capability::BackgroundExecution => (
                Action::UseBackgroundRuntime,
                "the browser profile executes while its page is alive; use a background runtime to continue after page teardown",
            ),
            Capability::Shell | Capability::ProcessSpawn | Capability::McpStdio => (
                Action::UseHostProcessRuntime,
                "the browser profile excludes host processes; use a host-process runtime for this capability",
            ),
            Capability::Hooks => (
                Action::UseHookRuntime,
                "the browser profile excludes configured hooks; use a hook-capable runtime",
            ),
            Capability::RuntimeSkills => (
                Action::UseSkillRuntime,
                "the browser profile excludes runtime skill discovery; use a skill-capable runtime",
            ),
            Capability::Schedule => (
                Action::UseScheduleRuntime,
                "the browser profile has no scheduler service; use a scheduler-capable runtime",
            ),
            Capability::WorkGraph => (
                Action::UseWorkGraphRuntime,
                "the browser profile has no WorkGraph service; use a WorkGraph-capable runtime",
            ),
            Capability::SemanticMemory => (
                Action::UseSemanticMemoryRuntime,
                "the browser profile has no semantic memory search store; use a semantic-memory-capable runtime",
            ),
            Capability::ImageGeneration => (
                Action::UseImageGenerationRuntime,
                "the browser profile has no image-generation tool executor; use an image-generation-capable runtime",
            ),
            Capability::FallbackWebSearch => (
                Action::UseWebSearchRuntime,
                "the browser profile has no fallback web-search executor; use a model with native search or a web-search-capable runtime",
            ),
            Capability::McpClient => (
                Action::UseMcpRuntime,
                "the browser profile excludes MCP clients; use an MCP-capable runtime",
            ),
            Capability::TcpComms | Capability::UdsComms => (
                Action::UseInProcessComms,
                "the browser profile supports in-process comms; use the inproc transport",
            ),
            Capability::RemoteMemberPlacement => (
                Action::UseLocalMemberPlacement,
                "the browser profile excludes remote member placement; omit placement to run the member locally",
            ),
        };
        Err(RuntimeProfileRefusal {
            code: RuntimeProfileRefusalCode::CapabilityUnavailable,
            message: message.to_owned(),
            data: RuntimeProfileRefusalData {
                profile: RuntimeProfileId::Browser,
                capability,
                clearing_action,
            },
        })
    }

    /// Apply the profile to a typed mobpack requirement. Unknown-token
    /// rejection remains owned by the manifest vocabulary gate.
    pub fn require_mobpack(
        self,
        capability: MobpackCapabilityId,
    ) -> Result<(), RuntimeProfileRefusal> {
        let capability = match capability {
            MobpackCapabilityId::Known(CapabilityId::Shell) => RuntimeProfileCapability::Shell,
            MobpackCapabilityId::Known(CapabilityId::Hooks) => RuntimeProfileCapability::Hooks,
            MobpackCapabilityId::Known(CapabilityId::Skills) => {
                RuntimeProfileCapability::RuntimeSkills
            }
            MobpackCapabilityId::Known(CapabilityId::McpLive) => {
                RuntimeProfileCapability::McpClient
            }
            MobpackCapabilityId::Known(CapabilityId::Schedule) => {
                RuntimeProfileCapability::Schedule
            }
            MobpackCapabilityId::Known(CapabilityId::WorkGraph) => {
                RuntimeProfileCapability::WorkGraph
            }
            MobpackCapabilityId::Known(CapabilityId::MemoryStore) => {
                RuntimeProfileCapability::SemanticMemory
            }
            MobpackCapabilityId::Known(CapabilityId::SessionStore) => {
                RuntimeProfileCapability::DurablePersistence
            }
            MobpackCapabilityId::HostProcess(HostProcessCapabilityId::ProcessSpawn) => {
                RuntimeProfileCapability::ProcessSpawn
            }
            MobpackCapabilityId::HostProcess(HostProcessCapabilityId::McpStdio) => {
                RuntimeProfileCapability::McpStdio
            }
            MobpackCapabilityId::Known(_)
            | MobpackCapabilityId::DeploySurface(_)
            | MobpackCapabilityId::Unknown => return Ok(()),
        };
        self.require(capability)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_browser_exclusion_has_a_typed_refusal_and_clearing_action() {
        use RuntimeProfileCapability as Capability;
        use RuntimeProfileClearingAction as Action;

        for (capability, clearing_action) in [
            (Capability::DurablePersistence, Action::UsePersistentRuntime),
            (
                Capability::BackgroundExecution,
                Action::UseBackgroundRuntime,
            ),
            (Capability::Shell, Action::UseHostProcessRuntime),
            (Capability::ProcessSpawn, Action::UseHostProcessRuntime),
            (Capability::McpStdio, Action::UseHostProcessRuntime),
            (Capability::Hooks, Action::UseHookRuntime),
            (Capability::RuntimeSkills, Action::UseSkillRuntime),
            (Capability::Schedule, Action::UseScheduleRuntime),
            (Capability::WorkGraph, Action::UseWorkGraphRuntime),
            (Capability::SemanticMemory, Action::UseSemanticMemoryRuntime),
            (
                Capability::ImageGeneration,
                Action::UseImageGenerationRuntime,
            ),
            (Capability::FallbackWebSearch, Action::UseWebSearchRuntime),
            (Capability::McpClient, Action::UseMcpRuntime),
            (Capability::TcpComms, Action::UseInProcessComms),
            (Capability::UdsComms, Action::UseInProcessComms),
            (
                Capability::RemoteMemberPlacement,
                Action::UseLocalMemberPlacement,
            ),
        ] {
            let refusal = BrowserRuntimeProfile
                .require(capability)
                .expect_err("excluded capability");
            assert_eq!(
                refusal.code,
                RuntimeProfileRefusalCode::CapabilityUnavailable
            );
            assert_eq!(refusal.data.profile, RuntimeProfileId::Browser);
            assert_eq!(refusal.data.capability, capability);
            assert_eq!(refusal.data.clearing_action, clearing_action);
            assert!(!refusal.message.is_empty());
        }
    }

    #[test]
    fn browser_profile_keeps_canonical_session_capabilities() {
        use RuntimeProfileCapability as Capability;

        for capability in [
            Capability::InMemoryPersistence,
            Capability::ForegroundExecution,
            Capability::KeepAlive,
            Capability::Comms,
            Capability::TransientTurnContext,
            Capability::EmbeddedSkills,
        ] {
            BrowserRuntimeProfile
                .require(capability)
                .expect("canonical session capability");
        }
    }

    #[test]
    fn excluded_mobpack_requests_use_the_same_profile_refusal() {
        use crate::MobpackCapabilityRequirement;
        use RuntimeProfileCapability as Capability;

        for (token, capability) in [
            ("shell", Capability::Shell),
            ("process_spawn", Capability::ProcessSpawn),
            ("mcp_stdio", Capability::McpStdio),
            ("hooks", Capability::Hooks),
            ("skills", Capability::RuntimeSkills),
            ("mcp_live", Capability::McpClient),
            ("session_store", Capability::DurablePersistence),
            ("schedule", Capability::Schedule),
            ("work_graph", Capability::WorkGraph),
            ("memory_store", Capability::SemanticMemory),
        ] {
            let requirement = MobpackCapabilityRequirement::parse(token);
            assert_eq!(
                BrowserRuntimeProfile.require_mobpack(requirement.id()),
                BrowserRuntimeProfile.require(capability),
            );
        }
    }
}
