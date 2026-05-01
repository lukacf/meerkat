use std::sync::Arc;

use meerkat_core::{
    AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, Provider,
};

fn fabricated<T>() -> T {
    panic!("compile-only fixture should never run")
}

async fn standalone_feature_entrypoint() {
    let builder = AgentBuilder::new();
    let client: Arc<dyn AgentLlmClient> = fabricated();
    let tools: Arc<dyn AgentToolDispatcher> = fabricated();
    let store: Arc<dyn AgentSessionStore> = fabricated();

    // SAFETY: this fixture models a downstream crate opting into the public
    // standalone feature and calling the live three-argument bypass shape. The
    // public method must not be callable from a production-like downstream
    // crate.
    #[allow(unsafe_code)]
    let _ = unsafe { builder.build_standalone(client, tools, store).await };
}

fn main() {
    let _ = Provider::OpenAI;
}
