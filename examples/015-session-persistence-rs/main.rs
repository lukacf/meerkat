//! # 015 — Session Persistence (Rust)
//!
//! Sessions can be saved to durable stores and loaded by ID. This example
//! writes to a temporary JSONL store and demonstrates a direct save/load
//! roundtrip; it does not restart a runtime-backed session host.
//!
//! ## What you'll learn
//! - JsonlStore vs MemoryStore vs SqliteSessionStore
//! - Saving and loading sessions
//! - Session lifecycle and low-level store inspection
//! - How JSONL compares with in-memory and SQLite session stores
//!
//! ## Run
//! ```bash
//! # From the repository root
//! ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
//!   --example 015-session-persistence --features jsonl-store
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, SessionFilter, SessionStore};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

type DemoError = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), DemoError> {
    meerkat_runtime::host_stack::run_host("015-session-persistence", async_main)?
}

async fn async_main() -> Result<(), DemoError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    // ── JsonlStore: file-based persistence ─────────────────────────────────
    println!("=== Backend 1: JsonlStore (file-based) ===\n");

    let jsonl_store = Arc::new(JsonlStore::new(store_dir.clone()));
    jsonl_store.init().await?;
    let adapted_store = Arc::new(StoreAdapter::new(jsonl_store.clone()));

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key.clone())?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .system_prompt("You are an assistant whose session is written to a store.")
        .max_tokens_per_turn(512)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), adapted_store)
        .await?;

    // Turn 1: Create session
    let result = agent
        .run("My project is called Phoenix. Remember that.".into())
        .await?;
    let session_id = result.session_id.clone();
    println!("Created session: {session_id}");
    println!("Response: {}\n", result.text);

    // Standalone automatic saves are best-effort. This demo requires an
    // acknowledged save and verifies the loaded identity and entire transcript.
    let loaded = save_and_verify(jsonl_store.as_ref(), agent.session()).await?;

    // List sessions from the store:
    let sessions = jsonl_store.list(SessionFilter::default()).await?;
    println!("Sessions on disk: {}", sessions.len());
    for s in &sessions {
        println!(
            "  {} — created: {:?}, messages: {}",
            s.id, s.created_at, s.message_count
        );
    }

    println!(
        "\nVerified stored session {} with {} messages",
        loaded.id(),
        loaded.messages().len()
    );

    // Turn 2 continues the original in-process agent, not a restarted host.
    let result = agent.run("What's my project called?".into()).await?;
    println!("\nTurn 2 response: {}", result.text);

    // ── Storage backend comparison ─────────────────────────────────────────

    println!("\n\n=== Storage Backend Comparison ===\n");
    println!(
        r#"| Backend          | Feature Flag    | Persistence | Best For                    |
|------------------|-----------------|-------------|-----------------------------|
| JsonlStore       | jsonl-store     | File (JSONL)| Development, simple deploy  |
| MemoryStore      | memory-store    | None (RAM)  | Tests, ephemeral agents     |
| SqliteSessionStore | session-store | SQLite/WAL  | Production, multi-session   |

SqliteSessionStore also supports:
- Session projection via SessionProjector
- Realm-based isolation for multi-tenant setups

# Feature flags in Cargo.toml:
[dependencies]
meerkat = {{ version = "0.8.24", features = ["jsonl-store", "session-store"] }}
"#
    );

    Ok(())
}

async fn save_and_verify(
    store: &dyn SessionStore,
    expected: &meerkat_core::Session,
) -> Result<meerkat_core::Session, DemoError> {
    store.save(expected).await?;
    let loaded = store.load(expected.id()).await?.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "saved session is missing")
    })?;
    if loaded.id() != expected.id()
        || serde_json::to_value(loaded.messages())? != serde_json::to_value(expected.messages())?
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "loaded session identity or transcript differs from the saved session",
        )
        .into());
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{Session, SessionId, SessionMeta, SessionStoreError};

    fn fixture() -> Session {
        let mut session = Session::new();
        session.append_external_user_content("My project is called Phoenix.".into());
        session
    }

    #[tokio::test]
    async fn jsonl_roundtrip_verifies_persisted_identity_and_content() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let store = JsonlStore::new(dir.path().to_path_buf());
        store.init().await.unwrap();
        let session = fixture();
        let loaded = save_and_verify(&store, &session).await.unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(
            serde_json::to_value(loaded.messages()).unwrap(),
            serde_json::to_value(session.messages()).unwrap()
        );
    }

    struct FaultyStore {
        fail_save: bool,
        load_id: Option<SessionId>,
        inner: meerkat_store::MemoryStore,
    }

    impl FaultyStore {
        async fn new(fail_save: bool, loaded: Option<Session>) -> Self {
            let inner = meerkat_store::MemoryStore::new();
            let load_id = loaded.as_ref().map(|session| session.id().clone());
            if let Some(session) = loaded {
                inner.save(&session).await.unwrap();
            }
            Self {
                fail_save,
                load_id,
                inner,
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for FaultyStore {
        async fn save(&self, _: &Session) -> Result<(), SessionStoreError> {
            if self.fail_save {
                Err(SessionStoreError::Internal("fixture save failure".into()))
            } else {
                Ok(())
            }
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(self.load_id.as_ref().unwrap_or(id)).await
        }

        async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
        }
    }

    #[tokio::test]
    async fn faulty_store_delegates_revision_guarded_delete() {
        let session = fixture();
        let store = FaultyStore::new(false, Some(session.clone())).await;
        let revision = meerkat_core::session_store::session_projection_cas_token(&session).unwrap();
        assert!(
            !store
                .delete_if_current_revision(session.id(), "stale")
                .await
                .unwrap()
        );
        assert!(store.load(session.id()).await.unwrap().is_some());
        assert!(
            store
                .delete_if_current_revision(session.id(), &revision)
                .await
                .unwrap()
        );
        assert!(store.load(session.id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_failed_wrong_identity_and_stale_saves_are_errors() {
        let session = fixture();
        let mut stale = session.clone();
        stale.append_external_user_content("Extra unsaved content".into());
        for (store, expected, reason) in [
            (FaultyStore::new(false, None).await, &session, "missing"),
            (
                FaultyStore::new(true, Some(session.clone())).await,
                &session,
                "fixture save failure",
            ),
            (
                FaultyStore::new(false, Some(fixture())).await,
                &session,
                "identity or transcript",
            ),
            (
                FaultyStore::new(false, Some(session.clone())).await,
                &stale,
                "identity or transcript",
            ),
        ] {
            let error = save_and_verify(&store, expected).await.unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
        }
    }
}
