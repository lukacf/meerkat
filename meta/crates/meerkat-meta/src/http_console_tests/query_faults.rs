#![allow(clippy::expect_used)]

use super::*;
use crate::console_aggregator::{
    AppendOutcome, ConsoleFrameSource, ConsoleFrameStatus, ConsoleTimelinePage,
    ConsoleTimelineWindowPage, InMemoryConsoleLogStore, NewConsoleFrame,
};
use axum::body::Body;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[derive(Clone, Copy, Debug)]
enum Fault {
    Read,
    Latest,
    NoProgress,
    Expired,
    Future,
}

struct FaultStore {
    inner: InMemoryConsoleLogStore,
    fault: Fault,
    latest_reads: AtomicUsize,
}

// A stalled page must contain a frame: empty pages are valid legacy terminals.
fn repeated_frame(cursor: Option<&ConsoleCursor>) -> ConsoleFrame {
    ConsoleFrame {
        id: "stalled-frame".into(),
        cursor: cursor.cloned().unwrap_or_else(|| "console:0".into()),
        dedupe_key: "stalled-frame".into(),
        timestamp_ms: 0,
        runtime_key: "fault-fixture".into(),
        identity: "router:main".into(),
        conversation_id: None,
        session_id: None,
        kind: "system_notice".into(),
        status: ConsoleFrameStatus::Completed,
        frame_version: 1,
        updated_at_ms: None,
        payload: serde_json::json!({ "text": "Repeated page" }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Synthetic,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for FaultStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
    }
    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(id, status).await
    }
    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        self.inner.query_frames(query).await
    }
    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        match self.fault {
            Fault::Read => Err(std::io::Error::other(
                "secret storage DSN; replay_unavailable is only text",
            )
            .into()),
            Fault::Expired => Err(Box::new(ConsoleTimelineQueryError::ReplayUnavailable {
                requested_cursor: query.after,
                latest_cursor: Some(ConsoleCursor::from_seq(42)),
            })),
            Fault::NoProgress => Ok(ConsoleTimelineWindowPage {
                frames: vec![repeated_frame(query.after.as_ref())],
                next_cursor: query.after,
                latest_cursor: Some(ConsoleCursor::from_seq(42)),
                exhausted: false,
            }),
            Fault::Latest | Fault::Future => self.inner.query_windowed_frames(query).await,
        }
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.latest_reads.fetch_add(1, Ordering::SeqCst);
        match self.fault {
            Fault::Latest => Err(std::io::Error::other("secret latest-cursor failure").into()),
            Fault::Future => Ok(None),
            _ => Ok(Some(ConsoleCursor::from_seq(42))),
        }
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }
    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime, kind, cursor)
            .await
    }
    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime, kind).await
    }
}

fn fixture(fault: Fault) -> (MetaConsoleAggregator, Arc<FaultStore>) {
    let store = Arc::new(FaultStore {
        inner: InMemoryConsoleLogStore::new(),
        fault,
        latest_reads: AtomicUsize::new(0),
    });
    (MetaConsoleAggregator::new(store.clone()), store)
}

fn query_request() -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: "meta/console/query_timeline".into(),
        params: json!({ "after": "console:1" }),
    }
}

#[tokio::test]
async fn timeline_faults_preserve_class_across_rest_sse_and_both_rpc_paths()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_temp, runtime) =
        super::tests::build_empty_console_test_runtime("timeline-faults").await?;
    for fault in [
        Fault::Read,
        Fault::Latest,
        Fault::NoProgress,
        Fault::Expired,
        Fault::Future,
    ] {
        let replay = matches!(fault, Fault::Expired | Fault::Future);
        for path in [
            "/console/timeline?after=console:1",
            "/console/timeline/stream?after=console:1",
        ] {
            let (aggregator, store) = fixture(fault);
            let app = console_json_router_with_aggregator(
                RuntimeDecisionState::local_console(
                    crate::ConsolePolicy {
                        require_app_auth: false,
                        ..Default::default()
                    },
                    None,
                ),
                aggregator,
            );
            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(Body::empty())?,
                )
                .await?;
            assert_eq!(
                response.status(),
                if replay {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                },
                "{fault:?} {path}"
            );
            let bytes = axum::body::to_bytes(response.into_body(), 4096).await?;
            let body: Value = serde_json::from_slice(&bytes)?;
            assert_eq!(
                body["error"],
                if replay {
                    "replay_unavailable"
                } else {
                    "timeline_unavailable"
                }
            );
            assert!(!body.to_string().contains("secret"));
            assert_eq!(
                store.latest_reads.load(Ordering::SeqCst),
                1,
                "no speculative latest read for {fault:?} {path}"
            );
        }
        for runtime_path in [false, true] {
            let (aggregator, store) = fixture(fault);
            let response = if runtime_path {
                Box::pin(handle_console_runtime_rpc(
                    &runtime,
                    None,
                    None,
                    None,
                    None,
                    Some(aggregator),
                    None,
                    None,
                    None,
                    query_request(),
                    true,
                ))
                .await
            } else {
                handle_console_aggregator_rpc(
                    Some(aggregator),
                    query_request(),
                    true,
                    false,
                    None,
                    None,
                    &crate::console_aggregator::ConsoleSendScope::Anonymous,
                )
                .await
            };
            assert_eq!(
                response["error"]["code"],
                if replay { -32013 } else { -32000 },
                "{fault:?} runtime={runtime_path}"
            );
            assert!(!response.to_string().contains("secret"));
            assert_eq!(store.latest_reads.load(Ordering::SeqCst), 1);
        }
    }
    runtime.handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn timeline_auth_failure_precedes_store_failure()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for path in [
        "/console/timeline?after=console:1",
        "/console/timeline/stream?after=console:1",
    ] {
        let (aggregator, store) = fixture(Fault::Latest);
        let app = console_json_router_with_aggregator(
            RuntimeDecisionState::local_console(crate::ConsolePolicy::default(), None),
            aggregator,
        );
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(store.latest_reads.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn approval_origin_access_cannot_be_bypassed_by_whitespace_ids()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let access = AccessController::new(crate::AccessControlConfig {
        enabled: true,
        admins: vec!["fixture-admin".into()],
        rules: vec![crate::AccessRule {
            id: "global-gating-only".into(),
            actions: vec![ACTION_GATING_VIEW.into(), ACTION_GATING_DECIDE.into()],
            ..Default::default()
        }],
        ..Default::default()
    })?;
    let definition = meerkat_mob::MobDefinition::from_toml(
        "[mob]\nid = 'approval-origin-access'\n[profiles.worker]\nmodel = 'gpt-5.5'\n",
    )?;
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .access_controller(access)
            .build(),
    )
    .await?;
    let mut pending_ids = Vec::new();
    for (conversation_id, interaction_id) in [
        (None, Some("private-turn".to_string())),
        (Some(" ".to_string()), Some("private-turn".to_string())),
        (
            Some("private-conversation".to_string()),
            Some("\t".to_string()),
        ),
    ] {
        let created = runtime
            .evaluate_gating_action_with_origin(
                crate::GatingEvaluateRequest {
                    action: "Private release".into(),
                    actor_id: "host".into(),
                    risk_tier: crate::GatingRiskTier::R3,
                    rationale: Some("Private rationale".into()),
                    requested_approver: None,
                    approval_recipient: None,
                    approval_channel: None,
                    approval_timeout_ms: Some(300_000),
                    entity: None,
                    topic: None,
                },
                Some(crate::GatingOrigin {
                    identity: "hidden-reviewer".into(),
                    conversation_id,
                    interaction_id,
                }),
            )
            .await;
        pending_ids.push(created.pending_id.expect("owner creates pending approval"));
    }
    let app = runtime.build_console_json_router(RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    ));
    for method in [
        "meta/gating/pending",
        "meta/gating/audit",
        "meta/gating/decide",
    ] {
        for id in pending_ids
            .iter()
            .flat_map(|pending_id| [pending_id.clone(), format!("  {pending_id}\t")])
        {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/console/rpc")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "jsonrpc":"2.0", "id":1, "method":method,
                    "params": {"pending_id":id, "approver_id":"operator", "decision":"approve"} })
                            .to_string(),
                        ))?,
                )
                .await?;
            let bytes = axum::body::to_bytes(response.into_body(), 32_768).await?;
            let body: Value = serde_json::from_slice(&bytes)?;
            if method.ends_with("/decide") {
                assert_eq!(body["error"]["data"]["kind"], "access_denied", "{body}");
            } else {
                assert!(body["error"].is_null(), "{body}");
                assert!(
                    !body.to_string().contains("Private"),
                    "hidden origin must not project: {body}"
                );
                assert!(!body.to_string().contains("private-turn"));
            }
            assert_eq!(
                runtime.list_gating_pending().await.len(),
                pending_ids.len(),
                "rejected route cannot mutate owner"
            );
        }
    }
    runtime.mob_runtime().handle().stop().await?;
    Ok(())
}
