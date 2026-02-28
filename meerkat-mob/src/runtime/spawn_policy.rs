use crate::MobRuntimeMode;
use crate::ids::{MeerkatId, ProfileName};
use async_trait::async_trait;

/// Specification for auto-spawning a member when a [`SpawnPolicy`] resolves a target.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub profile: ProfileName,
    pub runtime_mode: Option<MobRuntimeMode>,
}

/// Policy that determines whether an unknown meerkat ID should trigger an
/// automatic spawn.
///
/// Attached to the [`MobActor`] at build-time or set dynamically at runtime.
/// When a `send_message` (external turn) targets an unknown meerkat and a
/// spawn policy is present, the actor calls [`resolve`] to decide whether
/// to auto-provision a member.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SpawnPolicy: Send + Sync {
    /// Given an unknown meerkat ID, return a spawn spec if the policy
    /// should auto-spawn a member for it, or `None` to fall through
    /// to the normal `MeerkatNotFound` error.
    async fn resolve(&self, target: &MeerkatId) -> Option<SpawnSpec>;
}
