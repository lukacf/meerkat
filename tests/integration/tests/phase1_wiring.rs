use std::sync::Arc;

use meerkat::AgentFactory;
use meerkat_client::ProviderResolver;
use meerkat_core::{AgentSessionStore, Message, Provider, Session, UserMessage};
use meerkat_store::{MemoryStore, StoreAdapter};
use meerkat_tools::builtin::{BuiltinToolConfig, MemoryTaskStore};

#[tokio::test]
async fn test_agent_factory_builds_builtin_dispatcher() {
    let factory = AgentFactory::new("/tmp/meerkat-test-sessions").builtins(true);
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = factory
        .build_builtin_dispatcher(store, config, None, None, None)
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
    session.push(Message::User(UserMessage {
        content: "integration test".to_string(),
    }));

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

#[test]
fn test_provider_resolver_prefers_rkat_env() {
    let env = std::collections::HashMap::from([
        ("RKAT_OPENAI_API_KEY".to_string(), "rkat-key".to_string()),
        ("OPENAI_API_KEY".to_string(), "openai-key".to_string()),
    ]);
    let key = ProviderResolver::api_key_for_with_env(Provider::OpenAI, |key| env.get(key).cloned());
    assert_eq!(key.as_deref(), Some("rkat-key"));
}
