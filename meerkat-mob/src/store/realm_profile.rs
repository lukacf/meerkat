//! Realm-scoped reusable profile storage.

use super::MobStoreError;
use crate::profile::Profile;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// A realm-scoped reusable profile with revision metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredRealmProfile {
    /// Unique name within the realm.
    pub name: String,
    /// The profile definition.
    pub profile: Profile,
    /// Monotonic revision counter for CAS semantics.
    pub revision: u64,
    /// When this profile was first created.
    pub created_at: DateTime<Utc>,
    /// When this profile was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Trait for persisting and querying realm-scoped reusable profiles.
///
/// All mutation operations use CAS (compare-and-swap) semantics on the
/// revision counter to prevent lost updates.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealmProfileStore: Send + Sync {
    /// Create a new realm profile. Returns error if the name already exists.
    async fn create(
        &self,
        name: &str,
        profile: &Profile,
    ) -> Result<StoredRealmProfile, MobStoreError>;

    /// Get a realm profile by name.
    async fn get(&self, name: &str) -> Result<Option<StoredRealmProfile>, MobStoreError>;

    /// List all realm profiles.
    async fn list(&self) -> Result<Vec<StoredRealmProfile>, MobStoreError>;

    /// Update a realm profile. `expected_revision` must match the current revision.
    async fn update(
        &self,
        name: &str,
        profile: &Profile,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError>;

    /// Delete a realm profile. `expected_revision` must match the current revision.
    /// Returns the deleted profile on success.
    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError>;
}

// ---------------------------------------------------------------------------
// Reusable trait contract tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod contract_tests {
    use super::*;
    use crate::profile::ToolConfig;
    use crate::runtime_mode::MobRuntimeMode;

    fn sample_profile(model: &str) -> Profile {
        Profile {
            model: model.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: String::new(),
            external_addressable: false,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }
    }

    pub async fn test_create_and_get(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        let stored = store.create("worker", &profile).await.unwrap();
        assert_eq!(stored.name, "worker");
        assert_eq!(stored.profile.model, "claude-opus-4-8");
        assert_eq!(stored.revision, 1);
        assert!(stored.created_at <= Utc::now());
        assert!(stored.updated_at <= Utc::now());

        let fetched = store.get("worker").await.unwrap().unwrap();
        assert_eq!(fetched.name, "worker");
        assert_eq!(fetched.profile.model, "claude-opus-4-8");
        assert_eq!(fetched.revision, 1);
    }

    pub async fn test_get_nonexistent(store: &dyn RealmProfileStore) {
        let result = store.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    pub async fn test_create_duplicate_fails(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        store.create("dup", &profile).await.unwrap();
        let err = store.create("dup", &profile).await.unwrap_err();
        assert!(
            matches!(err, MobStoreError::CasConflict(_)),
            "expected CasConflict, got: {err:?}"
        );
    }

    pub async fn test_update_with_correct_revision(store: &dyn RealmProfileStore) {
        let profile_v1 = sample_profile("claude-opus-4-8");
        let created = store.create("evolve", &profile_v1).await.unwrap();
        assert_eq!(created.revision, 1);

        let profile_v2 = sample_profile("claude-sonnet-4-6");
        let updated = store.update("evolve", &profile_v2, 1).await.unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.profile.model, "claude-sonnet-4-6");
        assert!(updated.updated_at >= created.updated_at);

        let fetched = store.get("evolve").await.unwrap().unwrap();
        assert_eq!(fetched.profile.model, "claude-sonnet-4-6");
        assert_eq!(fetched.revision, 2);
    }

    pub async fn test_update_with_wrong_revision(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        store.create("stale", &profile).await.unwrap();

        let err = store.update("stale", &profile, 99).await.unwrap_err();
        assert!(
            matches!(err, MobStoreError::CasConflict(_)),
            "expected CasConflict, got: {err:?}"
        );
    }

    pub async fn test_update_nonexistent(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        let err = store.update("ghost", &profile, 1).await.unwrap_err();
        assert!(
            matches!(err, MobStoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    pub async fn test_delete_with_correct_revision(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        store.create("doomed", &profile).await.unwrap();

        let deleted = store.delete("doomed", 1).await.unwrap();
        assert_eq!(deleted.name, "doomed");
        assert_eq!(deleted.profile.model, "claude-opus-4-8");

        let fetched = store.get("doomed").await.unwrap();
        assert!(fetched.is_none());
    }

    pub async fn test_delete_with_wrong_revision(store: &dyn RealmProfileStore) {
        let profile = sample_profile("claude-opus-4-8");
        store.create("safe", &profile).await.unwrap();

        let err = store.delete("safe", 99).await.unwrap_err();
        assert!(
            matches!(err, MobStoreError::CasConflict(_)),
            "expected CasConflict, got: {err:?}"
        );

        // Should still exist
        assert!(store.get("safe").await.unwrap().is_some());
    }

    pub async fn test_delete_nonexistent(store: &dyn RealmProfileStore) {
        let err = store.delete("ghost", 1).await.unwrap_err();
        assert!(
            matches!(err, MobStoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    pub async fn test_list(store: &dyn RealmProfileStore) {
        let p1 = sample_profile("model-a");
        let p2 = sample_profile("model-b");
        store.create("alpha", &p1).await.unwrap();
        store.create("beta", &p2).await.unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    pub async fn test_list_empty(store: &dyn RealmProfileStore) {
        let all = store.list().await.unwrap();
        assert!(all.is_empty());
    }
}
