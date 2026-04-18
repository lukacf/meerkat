use std::sync::Arc;

use meerkat::AgentFactory;
use meerkat_core::{AgentSessionStore, Message, Session, UserMessage};
use meerkat_store::{MemoryStore, StoreAdapter};
use meerkat_tools::builtin::{BuiltinToolConfig, MemoryTaskStore};

#[tokio::test]
async fn test_agent_factory_builds_builtin_dispatcher() {
    let factory = AgentFactory::new("/tmp/meerkat-test-sessions").builtins(true);
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = factory
        .build_builtin_dispatcher(store, config, None, None, None, None, None)
        .await
        .expect("builtin dispatcher should build");

    let tools = dispatcher.tools();
    assert!(
        !tools.is_empty(),
        "builtin dispatcher should expose at least one tool"
    );
}

#[tokio::test]
async fn test_store_adapter_roundtrip() {
    let store = Arc::new(MemoryStore::new());
    let adapter = StoreAdapter::new(store);

    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "integration test".to_string(),
    )));

    let id = session.id().to_string();

    adapter.save(&session).await.expect("save should work");
    let loaded = adapter
        .load(&id)
        .await
        .expect("load should work")
        .expect("session should exist");

    assert_eq!(loaded.id(), session.id());
    assert_eq!(loaded.messages().len(), 1);
}

// Phase 6.6 removed the legacy env-precedence credential helper. The
// RKAT_*-preferred env resolution now lives inside the factory's
// env-var fallback and will migrate fully into ResolverEnvironment's
// env_lookup seam in later Phase 6 slices.
