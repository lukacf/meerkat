//! The pre-dequeue hook's fail-closed half, proven through the real runtime
//! loop.
//!
//! The positive half — a committed `brain_swap` request actually moving the
//! next provider call from model A to model B, under both WholeBlob and
//! HeadCanonical — lives in
//! `crates/meerkat/src/factory/brain_swap_runtime_loop_ab_tests.rs`. It has to live
//! in-crate because proving the ORDINARY registration path means substituting
//! a credential-free provider runtime in the factory's own private registry
//! slot, which no external test can reach and which no production caller needs
//! a setter for.
//!
//! This file proves the other half, which no positive test can: what happens
//! when a committed request CANNOT be proven. It builds a real runtime-backed
//! service over a real realm, materializes a session carrying a committed
//! `Requested` record bound to a run that never committed anything, and then submits the next input through the
//! ordinary admission path. Nothing here calls the realization method.
//!
//! It also proves the handle's precondition on the facade's persistent
//! executor: with such a request owed, a session whose host runtime stopped
//! has its registration degraded before the request is consulted, and the
//! next input is left queued for the session's next registration.
//!
//! # Why the seeding is out of band, and what that costs
//!
//! The only in-band writer of a committed request is the `brain_swap` builtin,
//! and it commits the request AS PART OF the originating run's clean boundary.
//! An unprovable request is therefore not constructible in band by
//! construction: the thing that makes it unprovable is the absence of the very
//! boundary that would have written it. Seeding it out of band is the only way
//! to reach this state, and it is a property of the state, not of the feature.
//!
//! That constraint is also why only HeadCanonical is covered here. WholeBlob
//! refuses an ordinary session write that would re-encode a store-owned
//! provisional candidate, so the out-of-band persist fails with
//! `ordinary WholeBlob write cannot bypass or re-encode a store-owned
//! provisional candidate` before the handoff is ever consulted. A WholeBlob
//! row here would then pass for entirely the wrong reason — a store-authority
//! conflict this fixture manufactured, not the pre-dequeue hold — which is
//! worse than no coverage. WholeBlob's in-band coverage is the A→B proof named
//! above.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use meerkat::surface::{
    build_runtime_backed_service_with_default_reconfigure_host, default_persistent_executor,
};
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
};
use meerkat_client::TestClient;
use meerkat_core::SessionBuildOptions;
use meerkat_core::image_generation::{
    SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin, SwitchTurnReasonTextDisposition,
    SwitchTurnRequestId,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_control::SessionModelRoutingControlRecord;
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{Input, InputLifecycleState, LogicalRuntimeId, MeerkatMachine, PromptInput};
use tokio::time::Duration;

async fn build_service(
    root: &std::path::Path,
    backend: meerkat_store::RealmBackend,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "brain-swap-realm",
        Some(backend),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let factory = AgentFactory::new(root.join("sessions"));
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    )));
    let (service, adapter) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        4,
        persistence,
        root.join("config_state.json"),
    )
    .expect("compose the runtime-backed service");
    (service, adapter)
}

fn create_request() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "gpt-5.4".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: meerkat::SystemPromptOverride::Set("pre-dequeue contract".to_string()),
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    }
}

async fn run_prompt(
    adapter: &Arc<MeerkatMachine>,
    session_id: &meerkat::SessionId,
    prompt: &str,
) -> CompletionOutcome {
    let (_outcome, handle) = adapter
        .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(prompt, None)))
        .await
        .expect("accept prompt input");
    let handle = handle.expect("completion handle");
    tokio::time::timeout(Duration::from_secs(20), handle.wait())
        .await
        .expect("prompt should settle in time")
        .expect("completion waiter should resolve")
}

fn until_changed_model_intent(target: &str) -> SwitchTurnIntent {
    SwitchTurnIntent {
        target_model: ModelId::new(target),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    }
}

/// A committed request whose originating run never committed a boundary must
/// hold, and the hold must block the pending input rather than serving it.
///
/// One healthy turn runs first, so the fixture also shows the loop was fine
/// until the unprovable handoff appeared: the second input is not blocked by
/// anything this harness did to the session, only by the seeded record.
async fn uncommitted_origin_blocks_the_next_input(backend: meerkat_store::RealmBackend) {
    let temp = tempfile::tempdir().expect("tempdir");
    let (service, adapter) = build_service(temp.path(), backend).await;

    let mut session = meerkat::Session::new();
    let session_id = session.id().clone();
    session
        .append_model_routing_control_record(
            SessionModelRoutingControlRecord::request(
                SwitchTurnRequestId::new(uuid::Uuid::from_bytes([42u8; 16])),
                RunId::new(),
                until_changed_model_intent("gpt-5.5"),
            )
            .expect("representable durable request"),
        )
        .expect("seed committed request");
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
    .expect("materialize session with committed unprovable request");

    // The next input must NOT be served: the loop's pre-dequeue pass observes a
    // committed request it cannot prove, and fails closed.
    let second = run_prompt(&adapter, &session_id, "second").await;
    assert!(
        !matches!(second, CompletionOutcome::Completed(_)),
        "an unprovable committed handoff must block the next input, got {second:?}"
    );
}

#[tokio::test]
async fn head_canonical_uncommitted_origin_blocks_the_next_input() {
    uncommitted_origin_blocks_the_next_input(meerkat_store::RealmBackend::Sqlite).await;
}

/// A runtime with named workers, standing for a host's or the process's.
fn named_runtime(name: &str) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(name)
        .enable_all()
        .build()
        .expect("build a runtime")
}

/// A committed request owed when the session's host runtime has stopped does
/// not end the next input.
///
/// The session is materialized on a host's runtime, which then stops while
/// the session is idle: the session's actor ended with it, while the runtime
/// loop, on Core's serving runtime, lives on. Another runtime of the process
/// admits the next input. The pre-dequeue handle's precondition asks the
/// session service first: the session's task ended with its runtime, so the
/// registration is degraded to a cold reload, nothing is realized, and the
/// caller is answered that the input stays durably admitted. The input stays
/// queued, never staged onto a run, for the session's next registration.
/// Without the precondition the handle consults the request, fails without
/// degrading anything, and the loop's cleanup ends the input.
#[test]
fn head_canonical_owed_handoff_after_the_host_runtime_stopped_leaves_the_input_queued() {
    let temp = tempfile::tempdir().expect("tempdir");
    let process = named_runtime("process-worker");
    let (service, adapter) = process.block_on(build_service(
        temp.path(),
        meerkat_store::RealmBackend::Sqlite,
    ));

    let mut session = meerkat::Session::new();
    let session_id = session.id().clone();
    session
        .append_model_routing_control_record(
            SessionModelRoutingControlRecord::request(
                SwitchTurnRequestId::new(uuid::Uuid::from_bytes([43u8; 16])),
                RunId::new(),
                until_changed_model_intent("gpt-5.5"),
            )
            .expect("representable durable request"),
        )
        .expect("seed committed request");
    // The session's actor is created on the host's runtime.
    let host = named_runtime("host-worker");
    {
        let service = Arc::clone(&service);
        let adapter = Arc::clone(&adapter);
        host.block_on(async move {
            let service_for_executor = Arc::clone(&service);
            let adapter_for_executor = Arc::clone(&adapter);
            Box::pin(meerkat::surface::materialize_session(
                &service,
                &adapter,
                session,
                create_request(),
                move |session_id| {
                    default_persistent_executor(
                        service_for_executor,
                        adapter_for_executor,
                        session_id,
                    )
                },
            ))
            .await
            .expect("materialize the session on the host's runtime");
        });
    }
    host.shutdown_timeout(Duration::from_secs(10));

    let observed = process.block_on(async move {
        let input = Input::Prompt(PromptInput::new(
            "the next input, after the host's runtime stopped",
            None,
        ));
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("the input is admitted");
        let handle = handle.expect("completion handle");
        let answered = tokio::time::timeout(Duration::from_secs(20), handle.wait()).await;
        let reload_required = adapter
            .durability_reload_required(&session_id)
            .await
            .is_some();
        let row = service
            .runtime_store()
            .load_input_state(&LogicalRuntimeId::for_session(&session_id), &input_id)
            .await
            .expect("the input's durable row is readable")
            .map(|row| {
                (
                    row.seed.phase,
                    row.seed.attempt_count,
                    row.seed.last_run_id.is_some(),
                )
            });
        drop((service, adapter));
        (matches!(answered, Ok(Err(_))), reload_required, row)
    });
    process.shutdown_timeout(Duration::from_secs(10));
    assert_eq!(
        observed,
        (true, true, Some((InputLifecycleState::Queued, 0, false))),
        "an input admitted after the session's host runtime stopped, with a committed request \
         owed, waits for the session's next registration: the registration must reload, and \
         the input stays queued and never staged onto a run"
    );
}
