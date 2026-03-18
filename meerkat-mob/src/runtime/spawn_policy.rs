use crate::MobRuntimeMode;
use crate::ids::{MeerkatId, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// Explicit owner for mutable runtime spawn-policy state.
#[derive(Default)]
pub struct SpawnPolicyService {
    policy: RwLock<Option<Arc<dyn SpawnPolicy>>>,
}

impl SpawnPolicyService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, policy: Option<Arc<dyn SpawnPolicy>>) {
        *self.policy.write().await = policy;
    }

    pub async fn resolve(&self, target: &MeerkatId) -> Option<SpawnSpec> {
        let policy = self.policy.read().await.clone();
        let policy = policy?;
        policy.resolve(target).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticPolicy;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SpawnPolicy for StaticPolicy {
        async fn resolve(&self, target: &MeerkatId) -> Option<SpawnSpec> {
            Some(SpawnSpec {
                profile: ProfileName::from(format!("role-{target}")),
                runtime_mode: Some(MobRuntimeMode::TurnDriven),
            })
        }
    }

    #[tokio::test]
    async fn spawn_policy_service_swaps_runtime_policy_authority() {
        let service = SpawnPolicyService::new();
        let target = MeerkatId::from("worker-1");
        assert!(service.resolve(&target).await.is_none());

        service.set(Some(Arc::new(StaticPolicy))).await;
        let resolved = service
            .resolve(&target)
            .await
            .expect("policy should resolve");
        assert_eq!(resolved.profile, ProfileName::from("role-worker-1"));
        assert_eq!(resolved.runtime_mode, Some(MobRuntimeMode::TurnDriven));
    }
}
