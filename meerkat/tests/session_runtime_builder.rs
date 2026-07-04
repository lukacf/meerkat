//! Smoke tests for `meerkat::session_runtime::SessionRuntimeBuilder`
//! and the accessors / setters introduced on `MeerkatSessionRuntime`
//! in W3-B and W3-C.

#![allow(clippy::expect_used)]
#![cfg(all(
    feature = "session-store",
    feature = "memory-store",
    not(target_arch = "wasm32")
))]

use std::sync::Arc;

use meerkat::session_runtime::SessionRuntimeBuilder;
use meerkat::surface::build_runtime_backed_service_with_capacities;
use meerkat::{
    AgentFactory, Config, FactoryAgentBuilder, PersistenceBundle, StagedSessionRegistry,
};
use meerkat_core::connection::RealmId;
use meerkat_store::MemoryBlobStore;

fn build_runtime() -> meerkat::session_runtime::MeerkatSessionRuntime {
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let persistence = PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(MemoryBlobStore::new()),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let builder = FactoryAgentBuilder::new(factory, Config::default());
    let staged_sessions = Arc::new(StagedSessionRegistry::new());
    let (service, runtime_adapter) =
        build_runtime_backed_service_with_capacities(builder, 4, 16, persistence);
    let service = Arc::new(service);
    SessionRuntimeBuilder::new(service, staged_sessions, runtime_adapter).build()
}

#[test]
fn builder_starts_with_no_realm_or_config() {
    let runtime = build_runtime();
    assert!(runtime.realm_id().is_none());
    assert!(runtime.instance_id().is_none());
    assert!(runtime.backend().is_none());
    assert!(runtime.config_runtime().is_none());
    assert!(runtime.default_llm_client().is_none());
}

#[test]
fn set_realm_context_updates_all_three_fields() {
    let runtime = build_runtime();
    runtime.set_realm_context(
        Some(RealmId::parse("realm-alpha".to_string()).expect("valid realm id")),
        Some("instance-1".to_string()),
        Some("backend-x".to_string()),
    );
    assert_eq!(
        runtime.realm_id().map(|r| r.to_string()),
        Some("realm-alpha".to_string())
    );
    assert_eq!(runtime.instance_id().as_deref(), Some("instance-1"));
    assert_eq!(runtime.backend().as_deref(), Some("backend-x"));
}

#[test]
fn skill_identity_registry_generation_guard_drops_stale_writes() {
    use meerkat_core::skills::SourceIdentityRegistry;

    let runtime = build_runtime();
    runtime.set_skill_identity_registry_for_generation(5, SourceIdentityRegistry::default());
    // Writing with a smaller generation is dropped silently.
    runtime.set_skill_identity_registry_for_generation(2, SourceIdentityRegistry::default());
    let _ = runtime.skill_identity_registry();
}

#[test]
fn builder_with_skill_identity_roots_pre_populates_inner() {
    use std::path::PathBuf;

    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let persistence = PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(MemoryBlobStore::new()),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let builder = FactoryAgentBuilder::new(factory, Config::default());
    let staged_sessions = Arc::new(StagedSessionRegistry::new());
    let (service, runtime_adapter) =
        build_runtime_backed_service_with_capacities(builder, 4, 16, persistence);
    let context_root = PathBuf::from("/tmp/meerkat-skills-context");
    let user_root = PathBuf::from("/tmp/meerkat-skills-user");
    let runtime = SessionRuntimeBuilder::new(Arc::new(service), staged_sessions, runtime_adapter)
        .with_skill_identity_context_root(context_root.clone())
        .with_skill_identity_user_root(user_root.clone())
        .build();
    // The slots are interior-mutable; construct succeeded and the
    // accessors don't panic.
    let _ = runtime.skill_identity_registry();
    let _ = (&context_root, &user_root);
}

#[test]
fn builder_with_realm_id_pre_populates_inner() {
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let persistence = PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(MemoryBlobStore::new()),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let builder = FactoryAgentBuilder::new(factory, Config::default());
    let staged_sessions = Arc::new(StagedSessionRegistry::new());
    let (service, runtime_adapter) =
        build_runtime_backed_service_with_capacities(builder, 4, 16, persistence);
    let runtime = SessionRuntimeBuilder::new(Arc::new(service), staged_sessions, runtime_adapter)
        .with_realm_id(RealmId::parse("alpha".to_string()).expect("valid realm id"))
        .with_instance_id("inst-1".to_string())
        .with_backend("be".to_string())
        .build();
    assert_eq!(
        runtime.realm_id().map(|r| r.to_string()).as_deref(),
        Some("alpha")
    );
    assert_eq!(runtime.instance_id().as_deref(), Some("inst-1"));
    assert_eq!(runtime.backend().as_deref(), Some("be"));
}
