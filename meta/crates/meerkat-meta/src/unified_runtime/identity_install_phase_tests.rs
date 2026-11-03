//! The identity-first install and bootstrap phases: an installed context is
//! visible (its public view answers, shutdown finds it) before anything is
//! registered, activated, restored or supervised; bootstrap then completes it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use crate::identity_first::{
    DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
    RosterContext, RosterError, RosterProvider,
};
use crate::runtime::RuntimeOptions;
use crate::{
    DiscoverySpec, InMemoryMetadataStore, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
};

use super::UnifiedRuntime;

struct EmptyRoster;

#[async_trait::async_trait]
impl RosterProvider for EmptyRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(Vec::new())
    }
}

pub(super) async fn runtime(temp: &tempfile::TempDir) -> UnifiedRuntime {
    // A distinct mob per test: parallel tests share the process-wide
    // participant name registry.
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"identity-install-phase-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let spec = MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        temp.path().to_path_buf(),
        4,
        None,
    )
    .expect("ephemeral spec")
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "identity-install-phase".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        Arc::new(InMemoryMetadataStore::new()),
    )
    .await
    .expect("bootstrap runtime")
}

pub(super) fn context(runtime: &UnifiedRuntime) -> Arc<IdentityFirstRuntimeContext> {
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "identity-install-phase".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(runtime.mob_handle()))),
        default_timeout: None,
    }));
    Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::new(EmptyRoster),
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
    ))
}

/// The installed context is the public identity view a host binds before
/// bootstrap; bootstrapping it afterwards completes and shuts down clean.
#[tokio::test]
async fn an_installed_context_is_visible_before_bootstrap_starts_anything() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    assert!(runtime.identity_runtime().is_none());

    let context = context(&runtime);
    runtime.install_identity_first_context(Arc::clone(&context));
    let view = runtime
        .identity_runtime()
        .expect("the installed identity view");
    assert!(
        std::ptr::eq(std::ptr::from_ref(&*view), Arc::as_ptr(&context.runtime)),
        "the installed runtime"
    );
    drop(view);

    runtime
        .bootstrap_installed_identity_first_context(&[])
        .await
        .expect("bootstrap the installed context");
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the bootstrapped context shuts down");
}

/// A host refusal between the phases leaves the ordinary shutdown to clean
/// the installed context up.
#[tokio::test]
async fn an_installed_context_refused_before_bootstrap_shuts_down_clean() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    runtime.install_identity_first_context(context(&runtime));
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
}

/// Bootstrap has nothing to complete without an installed context.
#[tokio::test]
async fn bootstrap_without_an_installed_context_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    assert!(
        runtime
            .bootstrap_installed_identity_first_context(&[])
            .await
            .is_err()
    );
    runtime.shutdown().await;
}
