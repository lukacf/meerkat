//! Facade-built surfaces apply durable job deliveries by default.
//!
//! The hosted composition every product surface uses arms the bundle's
//! delivery owner, and its sink admits an `Event` delivery through the
//! runtime's waking path, so an idle session runs it.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use meerkat::surface::{
    SessionServiceDeliverySink, build_runtime_backed_service_with_default_reconfigure_host,
    default_persistent_executor,
};
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, JobDeliveryApplication,
    JobDeliveryContent, JobDeliverySink, PersistentSessionService,
};
use meerkat_client::TestClient;
use meerkat_core::SessionBuildOptions;
use meerkat_runtime::terminal_status::InputTerminalReceiptWait;
use meerkat_runtime::{MeerkatMachine, RuntimeDeliveryOwnerAlreadyArmed, SessionServiceRuntimeExt};
use tokio::time::Duration;

async fn build_service(
    root: &std::path::Path,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
    meerkat::PersistenceBundle,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "delivery-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let factory = AgentFactory::new(root.join("sessions"));
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    )));
    let observer = persistence.clone();
    let (service, adapter) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        4,
        persistence,
        root.join("config_state.json"),
    );
    (service, adapter, observer)
}

fn create_request() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "gpt-5.4".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: meerkat::SystemPromptOverride::Set("delivery contract".to_string()),
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    }
}

#[tokio::test]
async fn the_hosted_composition_arms_the_bundle_delivery_owner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_service, _adapter, observer) = build_service(temp.path()).await;
    assert_eq!(
        observer
            .runtime_delivery_inbox()
            .claim_delivery_ownership()
            .err(),
        Some(RuntimeDeliveryOwnerAlreadyArmed),
        "the composition already owns the bundle's delivery inbox"
    );
    assert!(
        observer
            .runtime_delivery_owner()
            .arm(Arc::new(NoHost))
            .is_err(),
        "a second owner over the same bundle is refused"
    );
}

struct NoHost;

#[async_trait::async_trait]
impl meerkat::RuntimeDeliveryHost for NoHost {
    async fn delivery_sink(
        &self,
        _session_id: &meerkat::SessionId,
    ) -> Option<Arc<dyn JobDeliverySink>> {
        None
    }
}

#[tokio::test]
async fn an_event_delivery_wakes_an_idle_facade_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (service, adapter, _observer) = build_service(temp.path()).await;
    let session = meerkat::Session::new();
    let session_id = session.id().clone();
    let service_for_executor = Arc::clone(&service);
    let adapter_for_executor = Arc::clone(&adapter);
    Box::pin(meerkat::surface::materialize_session(
        &service,
        &adapter,
        session,
        create_request(),
        move |session_id| {
            default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
        },
    ))
    .await
    .expect("materialize an idle session");

    let job_id = meerkat::JobId::new("job-facade-event").expect("job id");
    let subscription = meerkat::JobSubscription::new(
        meerkat::JobSubscriptionId::new("watcher").expect("subscription id"),
        session_id.clone(),
        meerkat::JobDeliveryKind::Event {
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        },
    );
    let sink = SessionServiceDeliverySink::new(Arc::clone(&service), Arc::clone(&adapter));
    sink.apply(JobDeliveryApplication::Event {
        job_id: job_id.clone(),
        delivery_sequence: 1,
        subscription,
        interaction_lineage_id: meerkat::InteractionLineageId::new(),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        content: JobDeliveryContent::Terminal(meerkat::JobTerminalResult::Succeeded {
            result_ref: None,
        }),
    })
    .await
    .expect("event delivery applies");

    let admitted = adapter
        .input_state_by_idempotency_key(&session_id, &format!("job:{job_id}:1:watcher"))
        .await
        .expect("idempotency lookup")
        .expect("the event input is admitted under its delivery key");
    let wait = tokio::time::timeout(
        Duration::from_secs(20),
        adapter.wait_input_terminal_receipt(&session_id, &admitted.state.input_id),
    )
    .await
    .expect("the woken session runs the event input to a terminal")
    .expect("terminal receipt wait");
    assert!(
        matches!(wait, Some(InputTerminalReceiptWait::Resolved(_))),
        "the event input reached its terminal receipt: {wait:?}"
    );
}
