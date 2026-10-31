//! An identity runtime over a session bridge that cannot read what the
//! lifecycle projection reads: Core's member lifecycle journal (a host's
//! bridge that keeps the default reads, which answer `Unsupported`, or
//! implements only the head; a mob handle whose principal may not subscribe
//! to the mob's events) or an identity's binding in Core's member history.
//! No lifecycle side effect of a step Core journals can run then (the
//! customizer's `after_create`, the retirement hook, a retired session's
//! distillation and exit interview). That is never silent: the runtime
//! reports it typed once, and its health reads degraded for as long as it
//! lasts.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, header};
use tower::ServiceExt;

use meerkat_core::types::SessionId;

use super::held_identity_test_support::{SelectingHook, governed_runtime_with, member};
use crate::identity_first::bridge::SessionBridge;
use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
use crate::identity_first::{
    AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity, CustomizerError,
    DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
    MutableRosterProvider, RosterProvider,
};
use crate::mob_handle_runtime::SessionCreatedContext;
use crate::unified_runtime::ErrorHook;
use crate::unified_runtime::types::ErrorEvent;

/// How long the row waits for the report its first roster pass owes.
const REPORT_DEADLINE: Duration = Duration::from_secs(30);

/// How long the row waits, after the report, for a repeat that must not
/// come.
const SETTLE: Duration = Duration::from_millis(500);

/// The reports that the lifecycle projection is off among `events`, each in
/// its wire form.
fn projection_off_reports(events: &Mutex<Vec<ErrorEvent>>) -> Vec<serde_json::Value> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|event| serde_json::to_value(event).expect("an error event serializes"))
        .filter(|event| event["category"] == "lifecycle_projection_off")
        .collect()
}

/// An identity runtime over `bridge`, its error events kept in `events`.
fn identity_runtime_over(
    bridge: Arc<dyn SessionBridge>,
    instance: &str,
    events: &Arc<Mutex<Vec<ErrorEvent>>>,
) -> Arc<IdentityRuntime> {
    let identities = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }));
    let sink = Arc::clone(events);
    let hook: ErrorHook = Arc::new(move |event| {
        sink.lock().unwrap().push(event);
        Box::pin(async {})
    });
    identities.set_error_hook(Some(hook));
    identities
}

/// The reports that the projection is off, once `events` holds one and
/// nothing more came for [`SETTLE`]: exactly one.
async fn the_off_report(events: &Mutex<Vec<ErrorEvent>>) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + REPORT_DEADLINE;
    while projection_off_reports(events).is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the projection that is off was not reported within {REPORT_DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(SETTLE).await;
    let reports = projection_off_reports(events);
    assert_eq!(reports.len(), 1, "reported once: {reports:?}");
    assert!(
        reports[0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("lifecycle")),
        "the report says what is off: {reports:?}"
    );
    reports[0].clone()
}

/// A customizer that changes no build and answers every `after_create`.
struct QuietCustomizer;

#[async_trait::async_trait]
impl AgentCustomizer for QuietCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// The identity runtime starts over a bridge whose member lifecycle journal
/// reads answer `Unsupported`. Its first roster pass reports, once and typed
/// (`lifecycle_projection_off`), that lifecycle side effects are off; a later
/// pass reports nothing more. `meta/status` says the projection is off, and
/// the JSON health summary reads degraded and says why, while the
/// plain-text liveness answer stays `ok`.
///
/// RED before such a bridge was reported: the projection turned itself off
/// with a debug line only, so every lifecycle side effect was lost with no
/// report, and the runtime's status and health read as if nothing were
/// missing.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_bridge_without_journal_reads_is_reported_and_degrades_health() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut runtime = governed_runtime_with(
        &dir,
        "lifecycle-projection-off",
        &[],
        Arc::new(SelectingHook),
    )
    .await;
    // The bridge's members keep no journal: its journal reads answer
    // `Unsupported`, as a bridge that keeps the trait's defaults does.
    let events = Arc::new(Mutex::new(Vec::new()));
    let identities = identity_runtime_over(
        Arc::new(CoreReadsBridge::default()),
        "lifecycle-projection-off",
        &events,
    );
    let roster = Arc::new(MutableRosterProvider::new(Vec::new()));
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        Arc::clone(&identities),
        Arc::clone(&roster) as Arc<dyn RosterProvider>,
        None,
        None,
        None,
    ));
    runtime.install_identity_first_context(Arc::clone(&context));

    // The pass prepares the projection before it asks Core for anything.
    // This bridge answers none of the reads the rest of the pass makes, so
    // the pass itself may fail.
    let _ = context.refresh_desired_topology().await;
    // A later pass reports nothing more.
    let _ = context.refresh_desired_topology().await;
    the_off_report(&events).await;

    let status_context = crate::rpc::IdentityFirstContext {
        runtime: crate::identity_first::PublicIdentityRuntime::new(
            Arc::clone(&identities),
            runtime.mob_runtime().clone(),
        ),
        roster_provider: Arc::clone(&roster) as Arc<dyn RosterProvider>,
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: None,
        transcript_edit_service: None,
        compaction_floors: None,
    };
    let status: serde_json::Value = serde_json::from_str(
        &crate::rpc::handle_unified_rpc_json(
            &runtime,
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "meta/status"}).to_string(),
            Duration::from_secs(5),
            None,
            Some(&status_context),
        )
        .await,
    )
    .expect("meta/status answers JSON");
    assert_eq!(
        status["result"]["lifecycle_projection"]["state"], "off",
        "{status:#}"
    );

    let decisions = crate::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );
    let app = runtime.build_reference_app_router(decisions);
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .expect("a health request"),
        )
        .await
        .expect("a health answer");
    let health: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(health.into_body(), usize::MAX)
            .await
            .expect("the health body"),
    )
    .expect("the health summary is JSON");
    assert_eq!(health["status"], "degraded", "{health}");
    assert_eq!(health["lifecycle_projection"]["state"], "off", "{health}");
    let liveness = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("a liveness request"),
        )
        .await
        .expect("a liveness answer");
    assert_eq!(
        axum::body::to_bytes(liveness.into_body(), usize::MAX)
            .await
            .expect("the liveness body"),
        "ok"
    );
    let _ = runtime.shutdown().await;
}

/// A bridge that reads the head of Core's member lifecycle journal but none
/// of its pages turns the projection off, reported once and typed, rather
/// than leaving it ready and failing every read.
///
/// RED before such a bridge was reported: the projection read the head,
/// counted itself ready, and failed every page read after it, with a warning
/// at every sweep and no report.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_bridge_without_journal_pages_is_reported_off() {
    let bridge = Arc::new(CoreReadsBridge::default());
    bridge.core.keep_journal();
    bridge.refuse_journal_pages();
    let events = Arc::new(Mutex::new(Vec::new()));
    let identities = identity_runtime_over(
        Arc::clone(&bridge) as Arc<dyn SessionBridge>,
        "lifecycle-projection-no-pages",
        &events,
    );
    identities.settle_lifecycle_projection().await;
    identities.settle_lifecycle_projection().await;
    the_off_report(&events).await;
}

/// A bridge that reads Core's member lifecycle journal but not an identity's
/// binding in Core's member history turns the projection off once it reaches
/// a seat it must resolve, reported once and typed.
///
/// RED before such a bridge was reported: every page with a seat failed on
/// the history read, so the projection stayed ready and failed that page at
/// every sweep, with a warning and no report.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_bridge_without_binding_reads_is_reported_off() {
    let bridge = Arc::new(CoreReadsBridge::default());
    bridge.core.keep_journal();
    let events = Arc::new(Mutex::new(Vec::new()));
    let identities = identity_runtime_over(
        Arc::clone(&bridge) as Arc<dyn SessionBridge>,
        "lifecycle-projection-no-binding-reads",
        &events,
    );
    identities
        .set_agent_customizer(Some(Arc::new(QuietCustomizer)))
        .await;
    // The projection starts at the journal's head, before the seat.
    identities.settle_lifecycle_projection().await;
    let alice = AgentIdentity::parse("lifecycle-projection-alice").expect("identity");
    register_live(
        &identities,
        &bridge.core,
        member(&alice),
        0,
        SessionId::new(),
    )
    .await
    .expect("a live registered member");
    bridge.refuse_binding_reads();
    identities.settle_lifecycle_projection().await;
    identities.settle_lifecycle_projection().await;
    the_off_report(&events).await;
}

/// A bridge that reads Core's member lifecycle journal and member history
/// but not Core's standing of an identity's member turns the projection off
/// once it reaches a seat whose incarnation it must place, reported once and
/// typed.
///
/// RED before the projection read Core's standing: a seat's currency was
/// read from Core's member history, so nothing was reported, and the seat
/// was told from a read that can disagree with Core's standing.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_bridge_without_the_standing_read_is_reported_off() {
    let bridge = Arc::new(CoreReadsBridge::default());
    bridge.core.keep_journal();
    let events = Arc::new(Mutex::new(Vec::new()));
    let identities = identity_runtime_over(
        Arc::clone(&bridge) as Arc<dyn SessionBridge>,
        "lifecycle-projection-no-standing-read",
        &events,
    );
    identities
        .set_agent_customizer(Some(Arc::new(QuietCustomizer)))
        .await;
    // The projection starts at the journal's head, before the seat.
    identities.settle_lifecycle_projection().await;
    let alice = AgentIdentity::parse("lifecycle-projection-standing-alice").expect("identity");
    register_live(
        &identities,
        &bridge.core,
        member(&alice),
        0,
        SessionId::new(),
    )
    .await
    .expect("a live registered member");
    bridge.refuse_standing_reads();
    identities.settle_lifecycle_projection().await;
    identities.settle_lifecycle_projection().await;
    the_off_report(&events).await;
}

/// A session bridge over a mob handle whose principal may not subscribe to
/// the mob's events cannot read Core's member lifecycle journal: the
/// projection is off, reported once and typed.
///
/// RED before such a bridge was reported: the journal read was refused at
/// every sweep, each refusal a warning, with no report.
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_handle_that_may_not_read_the_journal_is_reported_off() {
    let dir = tempfile::tempdir().expect("temp dir");
    let runtime = governed_runtime_with(
        &dir,
        "lifecycle-projection-scope",
        &[],
        Arc::new(SelectingHook),
    )
    .await;
    // A principal that resolves to no scope.
    let handle =
        runtime
            .mob_handle()
            .with_command_authority(meerkat_mob::CommandAuthority::principal(
                meerkat_mob::MobControlPrincipal::Unresolved,
            ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let identities = identity_runtime_over(
        Arc::new(MobSessionBridge::new(handle)),
        "lifecycle-projection-scope",
        &events,
    );
    identities.settle_lifecycle_projection().await;
    identities.settle_lifecycle_projection().await;
    the_off_report(&events).await;
    let _ = runtime.shutdown().await;
}
