//! # 014 -- Semantic Memory (Rust)
//!
//! Semantic memory lets agents index and retrieve information outside the
//! current transcript. This runnable example uses an in-memory store; durable
//! cross-session memory uses the HNSW/SQLite implementation.
//!
//! ## What this example actually does
//! - Creates a `SimpleMemoryStore` (in-memory keyword-matching implementation)
//! - Wraps it in a `MemorySearchDispatcher` to expose the `memory_search` tool
//! - Composes the memory tool into the agent's tool dispatcher via `ToolGatewayBuilder`
//! - Pre-seeds the memory store with facts (simulating prior compaction indexing)
//! - Asks the agent to recall those facts using the `memory_search` tool
//!
//! ## What you'll learn
//! - Wiring a `MemoryStore` into an agent
//! - How `MemorySearchDispatcher` exposes `memory_search` as a tool
//! - How `ToolGatewayBuilder` composes multiple dispatchers
//! - The difference between `SimpleMemoryStore` (test) and `HnswMemoryStore` (production)
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat --example 014-semantic-memory --features jsonl-store,memory-store-session
//! ```

use std::sync::Arc;
use std::{io::Write, path::Path};

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, ToolGatewayBuilder};
use meerkat_core::MemoryIndexableContent;
use meerkat_core::memory::{
    MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemorySource, MemoryStore as _,
    MessageRange,
};
use meerkat_memory::{MemorySearchDispatcher, SimpleMemoryStore};
use meerkat_store::{JsonlStore, StoreAdapter};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("014-semantic-memory", async_main)?
}

async fn async_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let tmp = tempfile::tempdir_in(".")?;
    run_with_client(
        Arc::new(AnthropicClient::new(api_key)?),
        tmp.path(),
        &mut std::io::stdout(),
    )
    .await
}

async fn run_with_client(
    client: Arc<dyn meerkat_client::LlmClient>,
    directory: &Path,
    output: &mut (impl Write + Send),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let store_dir = directory.join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── Step 1: Create the memory store ──────────────────────────────────────
    //
    // SimpleMemoryStore is an in-memory implementation that uses keyword matching.
    // For production, use HnswMemoryStore which provides true vector-embedding
    // similarity search backed by hnsw_rs + SQLite persistence.

    let memory_store = Arc::new(SimpleMemoryStore::new());
    let mut memory_session = meerkat_core::Session::new();
    let memory_session_id = memory_session.id().clone();
    memory_session.set_session_metadata(meerkat_core::SessionMetadata {
        model_fallback: None,
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 1024,
        structured_output_retries: 2,
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: meerkat_core::SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        mob_member_binding: None,
    })?;
    memory_session.set_build_state(meerkat_core::SessionBuildState::default())?;

    // ── Step 2: Pre-seed memory with facts ───────────────────────────────────
    //
    // In normal operation, memory is populated during context compaction:
    // when the agent loop compacts old messages to save context space,
    // the discarded text is indexed into the memory store. Here we simulate
    // that by scoped indexing of some facts.

    let now = std::time::SystemTime::now();
    let facts = [
        "Our team uses Rust for all backend services, Python for data pipelines, \
         and TypeScript for the frontend.",
        "Deployment target is Kubernetes on AWS EKS. We deploy on Tuesdays and Thursdays.",
        "CI uses GitHub Actions. The staging environment is at staging.example.com.",
        "The database is PostgreSQL 16 with pgvector for embeddings.",
        "Team lead is Alice. Backend lead is Bob. Frontend lead is Carol.",
    ];

    for (i, fact) in facts.iter().enumerate() {
        let metadata = MemoryMetadata {
            session_id: memory_session_id.clone(),
            source: MemorySource::Compaction {
                source_range: MessageRange::single(i as u64),
            },
            indexed_at: now,
        };
        let request = MemoryIndexRequest::new(
            MemoryIndexScope::for_session(memory_session_id.clone()),
            MemoryIndexableContent::Indexable((*fact).to_string()),
            metadata,
        )?;
        memory_store.index_scoped(request).await?;
    }

    writeln!(
        output,
        "Indexed {} facts into SimpleMemoryStore\n",
        facts.len()
    )?;

    // ── Step 3: Create the memory search tool dispatcher ─────────────────────
    //
    // MemorySearchDispatcher wraps a MemoryStore and exposes it as the
    // `memory_search` tool. The agent can call this tool with a natural
    // language query and get back scored results.

    let memory_dispatcher = MemorySearchDispatcher::for_session(
        Arc::clone(&memory_store) as Arc<dyn meerkat_core::memory::MemoryStore>,
        memory_session_id,
    );

    // ── Step 4: Compose tool dispatchers ─────────────────────────────────────
    //
    // ToolGatewayBuilder merges multiple dispatchers into one. Here we combine
    // an empty base dispatcher with the memory search dispatcher. In a real
    // agent, the base would be a CompositeDispatcher with shell tools, tasks, etc.

    let base_tools: Arc<dyn meerkat_core::AgentToolDispatcher> =
        Arc::new(meerkat_tools::EmptyToolDispatcher);

    let gateway = ToolGatewayBuilder::new()
        .add_dispatcher(base_tools)
        .add_dispatcher(Arc::new(memory_dispatcher))
        .build()?;

    // ── Step 5: Build the agent with memory search wired in ─────────────────
    //
    // The ToolGateway contains MemorySearchDispatcher, so the agent can call
    // `memory_search` to retrieve indexed content while construction still
    // runs through the facade factory pipeline.

    let mut agent = AgentBuilder::new()
        .with_factory(factory)
        .model("claude-sonnet-4-6")
        .system_prompt(
            "You are a helpful assistant with access to a semantic memory store.\n\n\
             You have a `memory_search` tool that searches indexed memory.\n\
             Your memory contains facts representing earlier context that was \
             compacted away. When the user asks about things you don't see in \
             the current conversation, use `memory_search` to look them up.\n\n\
             Always use the memory_search tool when asked about team details, \
             infrastructure, or deployment information.",
        )
        .max_tokens_per_turn(1024)
        .resume_session(memory_session)
        .build(Arc::new(llm), Arc::new(gateway), store)
        .await?;

    // ── Step 6: Ask the agent to recall from memory ──────────────────────────

    writeln!(
        output,
        "=== Asking the agent to recall from semantic memory ===\n"
    )?;
    let result = agent
        .run(
            "What programming languages does our team use? \
             And what database do we run? Search your memory to find out."
                .into(),
        )
        .await?;
    writeln!(output, "Agent: {}\n", result.text)?;

    writeln!(output, "=== Asking about deployment schedule ===\n")?;
    let result = agent
        .run("When do we deploy and where? Check your memory.".into())
        .await?;
    writeln!(output, "Agent: {}\n", result.text)?;

    // ── Architecture reference ───────────────────────────────────────────────

    writeln!(output, "=== Semantic Memory Architecture ===\n")?;
    writeln!(
        output,
        r#"Memory data flow:

  Agent Loop
    |
    |-- (compaction) --> MemoryStore::index_scoped(request) --> scoped entries
    |
    |-- (tool call)  --> memory_search tool
                           |
                           v
                         MemoryStore::search(scope, query, limit) --> scoped results

Two implementations:
  - SimpleMemoryStore: In-memory, keyword matching (this example)
  - HnswMemoryStore:   Persistent, vector embeddings (hnsw_rs + SQLite)

Wiring in the factory (AgentFactory::build_agent), when memory is enabled:
  1. Creates HnswMemoryStore in <factory store_path>/memory/
  2. Passes it into the core agent loop for compaction indexing
  3. Wraps it in MemorySearchDispatcher for the memory_search tool
  4. Composes into ToolGateway alongside other dispatchers

Direct Rust embedding:
  AgentFactory::new(store_path).memory(true)

CLI usage:
  rkat run --tools full "What did I tell you about the API key?"
"#
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{Message, Provider, StopReason};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct MemoryClient {
        requests: Mutex<Vec<LlmRequest>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for MemoryClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            let mut events = Vec::new();
            assert_eq!(request.model, "claude-sonnet-4-6");
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "memory_search")
            );
            let stop_reason = match call {
                1 | 3 => {
                    events.push(Ok(LlmEvent::ToolCallComplete {
                        id: format!("recall-{call}"),
                        name: "memory_search".into(),
                        args: serde_json::json!({
                            "query": if call == 1 { "team database staging" } else { "deploy" },
                            "limit": 5
                        }),
                        meta: None,
                    }));
                    StopReason::ToolUse
                }
                2 | 4 => {
                    let Message::ToolResults { results, .. } = request.messages.last().unwrap()
                    else {
                        panic!("the real memory dispatcher must produce a tool result");
                    };
                    assert_eq!(results.len(), 1);
                    assert!(!results[0].is_error);
                    let rows: Vec<serde_json::Value> =
                        serde_json::from_str(&results[0].text_content()).unwrap();
                    assert_eq!(rows.len(), if call == 2 { 4 } else { 1 });
                    assert!(rows.iter().all(|row| row["source_range"]["start"].is_u64()));
                    let recalled = rows
                        .iter()
                        .map(|row| row["content"].as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join("\n");
                    for fact in if call == 2 {
                        vec![
                            "Rust",
                            "Python",
                            "TypeScript",
                            "PostgreSQL 16",
                            "staging.example.com",
                            "Alice",
                            "Bob",
                            "Carol",
                        ]
                    } else {
                        vec!["Kubernetes", "AWS EKS", "Tuesdays", "Thursdays"]
                    } {
                        assert!(recalled.contains(fact), "missing indexed fact: {fact}");
                    }
                    // Answers are derived from actual scoped search results, not fixture prose.
                    events.push(Ok(LlmEvent::TextDelta {
                        delta: recalled,
                        meta: None,
                    }));
                    StopReason::EndTurn
                }
                _ => panic!("unexpected request {call}"),
            };
            events.push(Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::Anthropic,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }));
            events.push(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }));
            Box::pin(futures::stream::iter(events))
        }

        fn provider(&self) -> Provider {
            Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[test]
    fn actual_demo_recalls_all_indexed_facts_through_same_session_tool_scope() {
        meerkat_runtime::host_stack::HostStackBudget::default_budget()
            .run("memory-demo-test", || async {
                let directory = tempfile::tempdir_in(".").unwrap();
                let path = directory.path().to_owned();
                let client = Arc::new(MemoryClient::default());
                let mut output = Vec::new();
                tokio::time::timeout(
                    Duration::from_secs(20),
                    run_with_client(client.clone(), &path, &mut output),
                )
                .await
                .unwrap()
                .unwrap();
                let requests = client.requests.lock().unwrap();
                assert_eq!(requests.len(), 4, "both turns must search, then answer");
                assert_eq!(
                    requests[2]
                        .messages
                        .iter()
                        .filter(|m| matches!(m, Message::User(_)))
                        .count(),
                    2,
                );
                let output = String::from_utf8(output).unwrap();
                assert!(output.contains("Indexed 5 facts"));
                assert!(output.contains("Agent: "));
                assert!(output.contains("PostgreSQL 16"));
                assert!(output.contains("Tuesdays"));
                assert!(output.contains("MemoryStore::index_scoped(request)"));
                drop(directory);
                assert!(
                    !path.exists(),
                    "example-owned session directory must be removed"
                );
            })
            .unwrap();
    }
}
