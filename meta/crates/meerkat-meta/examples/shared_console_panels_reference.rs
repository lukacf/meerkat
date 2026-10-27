//! Local, offline browser fixture for the shared native console.
//! Two repositories deliberately reuse the same member name. Their memory and
//! work graph records differ so switching scope can be verified in the UI.
//! Run with `scripts/repo-cargo run -p meerkat-meta --example shared_console_panels_reference`.
use std::sync::Arc;

use meerkat_meta::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleRuntimeRegistration, MetaConsoleAggregator,
};
use meerkat_meta::memory::records::{MemoryAuthor, MemoryKind};
use meerkat_meta::{
    AgentMemoryProvider, ConsolePolicy, MemoryScope, NewMemoryRecord, RuntimeDecisionState,
    SqliteAgentMemoryStore, UnifiedRuntime,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = tempfile::tempdir()?;
    let aggregator = MetaConsoleAggregator::in_memory();
    let mut runtimes = Vec::new();
    for name in ["alpha", "beta"] {
        let runtime = UnifiedRuntime::builder()
            .definition(meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = '{name}'\n[profiles.guide]\nmodel = 'gpt-6-astra'\nexternal_addressable = true\n[profiles.guide.tools]\ncomms = true\n",
            ))?)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await?;
        runtime
            .reconcile(vec![meerkat_mob::SpawnMemberSpec::new("guide", "guide")])
            .await?;
        let store = Arc::new(SqliteAgentMemoryStore::open(state.path().join(name))?);
        store
            .remember_authored(
                &MemoryScope::Identity {
                    realm: "default".into(),
                    identity: "guide".into(),
                },
                NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: format!("{name} repository boundary"),
                    description: "Offline browser fixture".into(),
                    body: format!("This fact belongs only to {name}."),
                    tags: Vec::new(),
                    evidence: Vec::new(),
                    verification: None,
                },
                MemoryAuthor::Operator,
            )
            .await?;
        runtime.set_memory_panel_store(store);
        runtime
            .workgraph_service()
            .ok_or("native work graph is unavailable")?
            .create(meerkat::CreateWorkItemRequest {
                title: format!("Inspect {name} repository"),
                ..Default::default()
            })
            .await?;
        let runtime = Arc::new(runtime);
        aggregator.register_runtime(ConsoleRuntimeRegistration {
            runtime_key: name.into(),
            identity_namespace: name.into(),
            runtime: runtime.clone(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        });
        runtimes.push(runtime);
    }
    let decisions = RuntimeDecisionState::local_console(
        ConsolePolicy {
            require_app_auth: false,
            read_only: true,
            ..Default::default()
        },
        None,
    );
    let app = meerkat_meta::http_console::console_frontend_router().merge(
        meerkat_meta::http_console::console_json_router_with_aggregator(decisions, aggregator),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!(
        "Shared console fixture: http://{}/console",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    for runtime in runtimes {
        runtime.mob_handle().stop().await?;
    }
    Ok(())
}
