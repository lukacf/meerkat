//! An identity runtime over a host's session bridge that keeps the default
//! member lifecycle journal reads, which answer `Unsupported`: no lifecycle
//! side effect of a step Core journals can run (the customizer's
//! `after_create`, the retirement hook, a retired session's distillation and
//! exit interview). That is never silent: the runtime reports it typed when
//! it starts, and its health reads degraded for as long as it lasts.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, header};
use tower::ServiceExt;

use super::held_identity_test_support::{SelectingHook, governed_runtime_with};
use crate::identity_first::core_test_bridge::CoreReadsBridge;
use crate::identity_first::{
    DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, MutableRosterProvider, RosterProvider,
};
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
    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "lifecycle-projection-off".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let hook: ErrorHook = Arc::new(move |event| {
        sink.lock().unwrap().push(event);
        Box::pin(async {})
    });
    identities.set_error_hook(Some(hook));
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
    let deadline = tokio::time::Instant::now() + REPORT_DEADLINE;
    while projection_off_reports(&events).is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the projection that is off was not reported within {REPORT_DEADLINE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = context.refresh_desired_topology().await;
    tokio::time::sleep(SETTLE).await;
    let reports = projection_off_reports(&events);
    assert_eq!(reports.len(), 1, "reported once: {reports:?}");
    assert!(
        reports[0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("lifecycle")),
        "the report says what is off: {reports:?}"
    );

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
