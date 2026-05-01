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

    // SAFETY: this fixture models a downstream crate fabricating the private
    // standalone authority argument through inference. The public method must
    // not be callable in this shape.
    #[allow(unsafe_code)]
    let standalone_authority = unsafe { std::mem::MaybeUninit::uninit().assume_init() };
    let _ = builder
        .build_standalone(standalone_authority, client, tools, store)
        .await;
}

fn main() {
    let _ = Provider::OpenAI;
}
