use crate::MobRuntimeMode;
use crate::definition::SpawnPolicyConfig;
use crate::ids::{AgentIdentity, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Specification for auto-spawning a member when a [`SpawnPolicy`] resolves a target.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub profile: ProfileName,
    pub runtime_mode: Option<MobRuntimeMode>,
}

/// Policy that determines whether an unknown agent identity should trigger an
/// automatic spawn.
///
/// Attached to the [`MobActor`] at build-time or set dynamically at runtime.
/// When a `send_message` (external turn) targets an unknown meerkat and a
/// spawn policy is present, the actor observes [`resolve`] and submits the
/// typed observation back to MobMachine before auto-provisioning a member.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SpawnPolicy: Send + Sync {
    /// Given an unknown agent identity, return a spawn spec if the policy
    /// should auto-spawn a member for it, or `None` to fall through
    /// to the normal `MeerkatNotFound` error.
    async fn resolve(&self, target: &AgentIdentity) -> Option<SpawnSpec>;
}

/// Runtime holder for the current dynamic spawn-policy callback.
///
/// This service is an observation source only. MobMachine owns whether policy
/// is enabled and records typed resolution feedback before any auto-spawn can
/// admit work for an unknown member.
#[derive(Default)]
pub struct SpawnPolicyService {
    policy: RwLock<Option<Arc<dyn SpawnPolicy>>>,
}

impl SpawnPolicyService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_policy(policy: Option<Arc<dyn SpawnPolicy>>) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    pub async fn set(&self, policy: Option<Arc<dyn SpawnPolicy>>) {
        *self.policy.write().await = policy;
    }

    pub async fn observe_resolution(&self, target: &AgentIdentity) -> Option<SpawnSpec> {
        let policy = self.policy.read().await.clone();
        let policy = policy?;
        policy.resolve(target).await
    }
}

#[derive(Debug, Clone)]
struct DefinitionSpawnPolicy {
    profile_map: BTreeMap<String, ProfileName>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SpawnPolicy for DefinitionSpawnPolicy {
    async fn resolve(&self, target: &AgentIdentity) -> Option<SpawnSpec> {
        self.profile_map
            .get(target.as_str())
            .map(|profile| SpawnSpec {
                profile: profile.clone(),
                runtime_mode: None,
            })
    }
}

pub(crate) fn definition_config_has_policy_fact(config: Option<&SpawnPolicyConfig>) -> bool {
    config.is_some()
}

pub(crate) fn definition_config_enables_policy(config: Option<&SpawnPolicyConfig>) -> bool {
    matches!(config, Some(SpawnPolicyConfig::Auto { .. }))
}

pub(crate) fn policy_from_definition_config(
    config: Option<&SpawnPolicyConfig>,
) -> Option<Arc<dyn SpawnPolicy>> {
    match config {
        Some(SpawnPolicyConfig::Auto { profile_map }) => Some(Arc::new(DefinitionSpawnPolicy {
            profile_map: profile_map.clone(),
        })),
        Some(SpawnPolicyConfig::None) | None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticPolicy;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SpawnPolicy for StaticPolicy {
        async fn resolve(&self, target: &AgentIdentity) -> Option<SpawnSpec> {
            Some(SpawnSpec {
                profile: ProfileName::from(format!("role-{target}")),
                runtime_mode: Some(MobRuntimeMode::TurnDriven),
            })
        }
    }

    #[tokio::test]
    async fn spawn_policy_service_reports_runtime_policy_observation() {
        let service = SpawnPolicyService::new();
        let target = AgentIdentity::from("worker-1");
        assert!(service.observe_resolution(&target).await.is_none());

        service.set(Some(Arc::new(StaticPolicy))).await;
        let resolved = service
            .observe_resolution(&target)
            .await
            .expect("policy should resolve");
        assert_eq!(resolved.profile, ProfileName::from("role-worker-1"));
        assert_eq!(resolved.runtime_mode, Some(MobRuntimeMode::TurnDriven));
    }

    #[tokio::test]
    async fn definition_policy_observation_uses_static_profile_map() {
        let target = AgentIdentity::from("reviewer");
        let mut profile_map = BTreeMap::new();
        profile_map.insert(target.as_str().to_owned(), ProfileName::from("lead"));

        let service = SpawnPolicyService::with_policy(policy_from_definition_config(Some(
            &SpawnPolicyConfig::Auto { profile_map },
        )));
        let resolved = service
            .observe_resolution(&target)
            .await
            .expect("definition policy should resolve mapped target");

        assert_eq!(resolved.profile, ProfileName::from("lead"));
        assert_eq!(resolved.runtime_mode, None);
        assert!(
            service
                .observe_resolution(&AgentIdentity::from("missing"))
                .await
                .is_none()
        );
    }
}
