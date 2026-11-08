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
    )
    .expect("compose the runtime-backed service");
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
    async fn delivery_route(
        &self,
        _session_id: &meerkat::SessionId,
    ) -> Option<meerkat::DeliveryRoute> {
        None
    }

    async fn claim_cold_delivery(
        &self,
        _session_id: &meerkat::SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
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

// This service injects an already-decided typed control result. The test below
// exercises the actual Notification sink, not native review adjudication or
// authenticated context-control admission.
struct ReviewControlService {
    refusal: Option<meerkat_core::approval::review::OperationReviewRefusal>,
    calls: std::sync::Mutex<Vec<(meerkat::SessionId, meerkat_core::AppendSystemContextRequest)>>,
}

#[async_trait::async_trait]
impl meerkat_core::SessionService for ReviewControlService {
    async fn create_session(
        &self,
        _req: CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, meerkat_core::SessionError> {
        panic!("Notification must not create a session")
    }

    async fn start_turn(
        &self,
        _id: &meerkat::SessionId,
        _req: meerkat_core::StartTurnRequest,
    ) -> Result<meerkat_core::RunResult, meerkat_core::SessionError> {
        panic!("Notification must not start a turn")
    }

    async fn interrupt(&self, _id: &meerkat::SessionId) -> Result<(), meerkat_core::SessionError> {
        panic!("Notification must not interrupt a turn")
    }

    async fn read(
        &self,
        _id: &meerkat::SessionId,
    ) -> Result<meerkat_core::SessionView, meerkat_core::SessionError> {
        panic!("Notification must use the control owner")
    }

    async fn list(
        &self,
        _query: meerkat_core::SessionQuery,
    ) -> Result<Vec<meerkat_core::SessionSummary>, meerkat_core::SessionError> {
        panic!("Notification must not search for another recipient")
    }

    async fn archive(&self, _id: &meerkat::SessionId) -> Result<(), meerkat_core::SessionError> {
        panic!("Notification must not archive a recipient")
    }
}

#[async_trait::async_trait]
impl meerkat_core::SessionServiceControlExt for ReviewControlService {
    async fn append_system_context(
        &self,
        id: &meerkat::SessionId,
        request: meerkat_core::AppendSystemContextRequest,
    ) -> Result<meerkat_core::AppendSystemContextResult, meerkat_core::SessionControlError> {
        self.calls
            .lock()
            .expect("control calls")
            .push((id.clone(), request));
        if let Some(refusal) = self.refusal {
            return Err(meerkat_core::SessionControlError::Review(refusal));
        }
        Ok(meerkat_core::AppendSystemContextResult {
            status: meerkat_core::AppendSystemContextStatus::Applied,
        })
    }
}

#[tokio::test]
async fn notification_sink_preserves_exact_review_refusal_and_permitted_control() {
    use meerkat_core::approval::review::{
        OperationReviewRefusal, ReviewUnavailableKind, ReviewUnsatisfiedKind,
    };

    let runtime = Arc::new(MeerkatMachine::ephemeral());
    for refusal in [
        None,
        Some(OperationReviewRefusal::Unavailable {
            kind: ReviewUnavailableKind::UnsupportedEntry,
        }),
        Some(OperationReviewRefusal::Unsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
        }),
    ] {
        let service = Arc::new(ReviewControlService {
            refusal,
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let sink = SessionServiceDeliverySink::new(service.clone(), runtime.clone());
        let session_id = meerkat::SessionId::new();
        let job_id = meerkat::JobId::new("job-review-notification").expect("job id");
        let subscription = meerkat::JobSubscription::new(
            meerkat::JobSubscriptionId::new("watcher").expect("subscription id"),
            session_id.clone(),
            meerkat::JobDeliveryKind::Notification,
        );
        let content = JobDeliveryContent::Terminal(meerkat::JobTerminalResult::Succeeded {
            result_ref: None,
        });
        let expected_request =
            meerkat::job_delivery_notification_request(&job_id, 1, &subscription, &content);
        let result = sink
            .apply(JobDeliveryApplication::Notification {
                job_id,
                delivery_sequence: 1,
                subscription,
                content,
            })
            .await;
        assert_eq!(
            result,
            refusal.map_or(Ok(()), |error| {
                Err(meerkat::JobDeliveryApplyError::Review(error))
            })
        );
        let calls = service.calls.lock().expect("control calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, session_id);
        assert_eq!(
            serde_json::to_value(&calls[0].1).expect("actual request"),
            serde_json::to_value(&expected_request).expect("expected request"),
            "the exact recipient and Notification request reach the control owner"
        );
    }
}

/// The incarnation of the mob `team` the member rows below are bound to.
fn team_incarnation() -> meerkat_core::service::MobIncarnationId {
    "6a1d3c58-2f0e-4b7a-9c41-8e5d2b0f7a13"
        .parse()
        .expect("canonical mob incarnation id")
}

/// A mob member resolver double, as a host's mob runtime binds one: every
/// member address is served by `session`, and the member's mob refuses
/// external work for it, as a mob refuses it for a member that became an
/// identity entry.
struct RefusesExternalWork {
    session: meerkat::SessionId,
}

#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for RefusesExternalWork {
    async fn current_address(
        &self,
        owner: &meerkat::ContinuationOwner,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        let meerkat::ContinuationOwner::Member { mob_id, identity } = owner else {
            return Err("a member resolver".into());
        };
        meerkat::member_delivery_address(mob_id, team_incarnation(), identity, 1)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    async fn resolve_address(
        &self,
        address: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<meerkat::AddressResolution, String> {
        Ok(
            match address
                .member_address()
                .map_err(|error| error.to_string())?
            {
                Some(_) => meerkat::AddressResolution::Session(self.session.clone()),
                None => meerkat::default_address_resolution(address),
            },
        )
    }

    async fn external_work_refusal(
        &self,
        _address: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::RuntimeDeliveryRefusalReason>, String> {
        Ok(Some(
            meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
        ))
    }
}

/// A job owner that owns no job.
struct NoRetainedJobs;

#[async_trait::async_trait]
impl meerkat::RetainedJobSource for NoRetainedJobs {
    async fn retained_job(
        &self,
        _producer: &meerkat::ContinuationProducer,
        _job_id: &str,
    ) -> meerkat::RetainedJobLookup {
        meerkat::RetainedJobLookup::Absent
    }
}

/// External work a caller committed to a mob member (a keyed member send
/// through a surface's external door) is asked again at its delivery by the
/// hosted composition's delivery host, which forwards the question to the
/// continuation services the host's mob runtime binds. A refusal settles the
/// row refused for good, and nothing is admitted into the member's session.
#[tokio::test]
async fn external_work_a_member_resolver_refuses_settles_refused_through_the_hosted_composition() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (service, adapter, observer) = build_service(temp.path()).await;
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
    .expect("materialize the member's idle session");

    let resolver = Arc::new(RefusesExternalWork {
        session: session_id.clone(),
    });
    let _binding = observer
        .continuation_bindings()
        .bind(resolver.clone(), Arc::new(NoRetainedJobs))
        .expect("the host binds its continuation services");
    let continuations = meerkat::ContinuationOwnerService::new(
        observer.runtime_delivery_inbox(),
        resolver,
        Arc::clone(&adapter),
    );
    let owner = meerkat::ContinuationOwner::Member {
        mob_id: "team".into(),
        identity: "worker".into(),
    };
    let key = meerkat::ContinuationKey::new("external-1").expect("key");
    let receipt = continuations
        .submit_external_work_for_member_incarnation(
            &owner,
            team_incarnation(),
            meerkat::ContinuationDelivery {
                key: key.clone(),
                result: meerkat::ContinuationResultRef {
                    producer: meerkat::ContinuationProducer::Host {
                        namespace: "tasks".into(),
                    },
                    producer_id: "op-1".into(),
                    result_digest: "sha256:result".into(),
                    summary: None,
                },
                body: "external work".into(),
                handling: meerkat::ContinuationHandling::Queue,
            },
            1,
        )
        .await
        .expect("the external work commits");

    let settled = tokio::time::timeout(Duration::from_mins(1), async {
        loop {
            let status = continuations
                .continuation_status(&owner, &key)
                .await
                .expect("status");
            if !matches!(status, meerkat::ContinuationStatus::Pending { .. }) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the row settles");
    assert!(
        matches!(
            &settled,
            meerkat::ContinuationStatus::Refused {
                reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
                ..
            }
        ),
        "the delivery host asks the bound resolver, and its refusal settles the row: {settled:?}"
    );
    assert!(
        adapter
            .input_state_by_idempotency_key(&session_id, &receipt.delivery_id)
            .await
            .expect("idempotency lookup")
            .is_none(),
        "nothing was admitted into the member's session"
    );
}
