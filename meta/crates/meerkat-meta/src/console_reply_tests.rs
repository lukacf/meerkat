//! Thread references and person-addressed private replies: a console send
//! records where it came from (the member, the interaction, who may read the
//! replies to it and the person they are for), and a reply meant for the one
//! person who made a request reaches only that person: in their own private
//! thread, or as an item of their attention list, never in a shared
//! conversation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, state};
use super::console_side_chat_tests::seeded_member;
use super::console_token_lapse_tests::{console_rpc_as, now_seconds, token};
use super::*;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, AppendOutcome, ConsoleCursor, ConsoleDeltaCompaction,
    ConsoleFrame, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogResult, ConsoleLogStore,
    ConsoleSendDedupeDurability, ConsoleThreadAudience, ConsoleThreadReference,
    ConsoleTimelinePage, ConsoleTimelineQuery, ConsoleTimelineWindowPage,
    ConsoleTimelineWindowQuery, ConsoleVisibilityPolicy, InMemoryConsoleLogStore, NewConsoleFrame,
};
use crate::console_replies::{
    CONSOLE_ATTENTION_PAGE, ConsoleReplies, ConsoleReplyDelivery, ConsoleReplyError,
    ConsoleReplyReceipt,
};
use crate::runtime::{MetadataStoreError, PersistentMetadataStore};
use crate::unified_runtime::{UnifiedRuntime, UnifiedRuntimeBuilder};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

const ALICE: &str = "alice@example.test";
const ROOT: &str = "root@example.test";
const ROBOT: &str = "svc:robot";

/// The issuer of every token the rows present.
const ISSUER: &str = "https://trusted.meta.localhost";

/// The text of every reply the rows deliver: no frame of a send or a turn
/// carries it, so a frame or a stream event that does is the reply's.
const REPLY_TEXT: &str = "The released answer: 42 (private reply marker 7f3a).";

/// What [`WithholdReplies`] leaves of a private reply.
const WITHHELD: &str = "[withheld by policy]";

const PROFILES: &str = r#"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#;

fn bearer(subject: &str, email: &str) -> String {
    token(
        json!({ "sub": subject, "email": email, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    )
}

fn alice() -> String {
    bearer("oidc-sub-alice", ALICE)
}

fn bob() -> String {
    bearer("oidc-sub-bob", BOB)
}

fn root() -> String {
    bearer("oidc-sub-root", ROOT)
}

fn robot() -> String {
    token(
        json!({ "sub": ROBOT, "email": ROBOT, "actor_type": "service" }),
        Some(now_seconds() + 3600),
    )
}

/// An access configuration under which every signed-in caller may do
/// everything to every agent, with root its one admin.
fn everyone_drives_every_agent() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// An access configuration that grants no one anything: only root, its
/// admin, sees the agents.
fn no_one_drives_any_agent() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: Vec::new(),
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// A console payload policy that withholds the text of every private reply
/// frame.
struct WithholdReplies;

impl ConsoleVisibilityPolicy for WithholdReplies {
    fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
        (frame.kind == "private_reply").then(|| json!({ "text": WITHHELD }))
    }
}

/// The console decisions of the rows: Alice and root allowlisted beside the
/// base people; `sign_in` is the Enforced posture, `read_only` a console
/// that takes no changes.
fn decisions(sign_in: bool, read_only: bool) -> crate::runtime::RuntimeDecisionState {
    let mut decisions = state().decisions;
    decisions
        .auth
        .email_allowlist
        .extend([ALICE.to_string(), ROOT.to_string()]);
    decisions.console.require_app_auth = sign_in;
    decisions.console.read_only = read_only;
    decisions
}

/// A composition over `dir` whose sessions persist (a side chat is a durable
/// fork), its builder finished by `stores` (which may install the stores a
/// row observes). The agent `alpha` is seeded by the caller, once per state
/// directory.
async fn composition(
    dir: &std::path::Path,
    mob_id: &str,
    access: Option<AccessController>,
    stores: impl FnOnce(UnifiedRuntimeBuilder) -> UnifiedRuntimeBuilder,
) -> Result<UnifiedRuntime, BoxError> {
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n{PROFILES}"
        ))?)
        .persistent_state(dir)
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()));
    if let Some(access) = access {
        builder = builder.access_controller(access);
    }
    Ok(stores(builder)
        .build()
        .await
        .map_err(|error| error.to_string())?)
}

/// A console over a composition with the agent `alpha`, its console router
/// built by the composition itself. Sign-in on is the Enforced posture for
/// Alice, Bob, root and the robot service; off is an open console.
struct Fixture {
    _dir: tempfile::TempDir,
    owner: UnifiedRuntime,
    app: Router,
}

impl Fixture {
    async fn new(
        mob_id: &str,
        sign_in: bool,
        access: Option<AccessController>,
    ) -> Result<Self, BoxError> {
        Self::build(mob_id, sign_in, access, None, |builder| builder).await
    }

    /// The console, with `host_policy` as the composition's host payload
    /// policy when one is given (it applies before frames are stored).
    async fn with_host_policy(
        mob_id: &str,
        sign_in: bool,
        access: Option<AccessController>,
        host_policy: Option<Arc<dyn ConsoleVisibilityPolicy>>,
    ) -> Result<Self, BoxError> {
        Self::build(mob_id, sign_in, access, host_policy, |builder| builder).await
    }

    /// The console over a composition whose builder `stores` finishes.
    async fn build(
        mob_id: &str,
        sign_in: bool,
        access: Option<AccessController>,
        host_policy: Option<Arc<dyn ConsoleVisibilityPolicy>>,
        stores: impl FnOnce(UnifiedRuntimeBuilder) -> UnifiedRuntimeBuilder,
    ) -> Result<Self, BoxError> {
        let dir = tempfile::tempdir()?;
        let owner = composition(dir.path(), mob_id, access, stores).await?;
        if let Some(policy) = host_policy {
            owner.initialize_console_projection_with_policy(policy)?;
        }
        seeded_member(owner.mob_runtime(), "worker", "alpha", &[]).await?;
        let app = owner.build_console_json_router_with_policy(
            decisions(sign_in, false),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        Ok(Self {
            _dir: dir,
            owner,
            app,
        })
    }

    async fn enforced(mob_id: &str) -> Result<Self, BoxError> {
        Self::new(mob_id, true, Some(everyone_drives_every_agent())).await
    }

    /// Another console router over the same composition, with its own
    /// decisions and its own payload policy.
    fn router(
        &self,
        decisions: crate::runtime::RuntimeDecisionState,
        policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) -> Router {
        self.owner
            .build_console_json_router_with_policy(decisions, policy)
    }

    /// The composition's person-addressed delivery: what a host's reply
    /// route calls.
    fn replies(&self) -> ConsoleReplies {
        self.owner
            .console_replies()
            .expect("a composition with a console router delivers replies")
    }

    async fn stop(self) {
        let _ = self.owner.shutdown().await;
    }
}

/// `POST /console/send` of a question to `identity` as the holder of
/// `bearer` (no token on an open console): the accepted send.
async fn send(app: &Router, bearer: Option<&str>, identity: &str, key: &str) -> Value {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/send")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    json!({
                        "identity": identity,
                        "content": format!("What does the report say? ({key})"),
                        "origin": "console",
                        "idempotency_key": key,
                    })
                    .to_string(),
                ))
                .expect("send request"),
        )
        .await
        .expect("send response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("send body");
    let accepted: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "the send is accepted: {accepted:#}");
    accepted
}

fn interaction_of(accepted: &Value) -> String {
    accepted["interaction_id"]
        .as_str()
        .expect("an accepted send names its interaction")
        .to_string()
}

/// `meta/fork_member` of `source` into the side chat `side_chat` as the
/// holder of `bearer`, retried while the source still settles its last turn.
async fn fork_side_chat(app: &Router, bearer: &str, source: &str, side_chat: &str) {
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    let mut answer = Value::Null;
    for _ in 0..50 {
        answer = console_rpc_as(app, Some(bearer), "meta/fork_member", params.clone()).await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        answer["error"].is_null(),
        "the side chat is forked: {answer:#}"
    );
}

/// The `caller_scope` `meta/console/whoami` reports to the holder of
/// `bearer`.
async fn caller_scope(app: &Router, bearer: &str) -> String {
    let whoami = console_rpc_as(app, Some(bearer), "meta/console/whoami", json!({})).await;
    whoami["result"]["caller_scope"]
        .as_str()
        .expect("whoami reports the caller scope")
        .to_string()
}

/// The frames of `identity` (every identity when `None`) the holder of
/// `bearer` reads, or the answer's error.
async fn timeline(app: &Router, bearer: &str, identity: Option<&str>) -> Result<Vec<Value>, Value> {
    let mut params = json!({ "limit": 500 });
    if let Some(identity) = identity {
        params["identity"] = json!(identity);
    }
    let answer = console_rpc_as(app, Some(bearer), "meta/console/query_timeline", params).await;
    if !answer["error"].is_null() {
        return Err(answer["error"].clone());
    }
    Ok(answer["result"]["frames"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn carries_reply(value: &Value) -> bool {
    value.to_string().contains(REPLY_TEXT)
}

/// The attention items the holder of `bearer` reads, or the answer's error.
async fn attention(app: &Router, bearer: &str) -> Result<Vec<Value>, Value> {
    let answer = console_rpc_as(app, Some(bearer), "meta/console/attention", json!({})).await;
    if !answer["error"].is_null() {
        return Err(answer["error"].clone());
    }
    Ok(answer["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn is_attention(receipt: &Result<ConsoleReplyReceipt, ConsoleReplyError>) -> bool {
    matches!(
        receipt,
        Ok(receipt) if matches!(receipt.delivery, ConsoleReplyDelivery::Attention { .. })
    )
}

fn is_thread(receipt: &Result<ConsoleReplyReceipt, ConsoleReplyError>) -> bool {
    matches!(
        receipt,
        Ok(receipt) if matches!(receipt.delivery, ConsoleReplyDelivery::Thread { .. })
    )
}

/// One open `GET /console/timeline/stream` as a client reads it.
struct TimelineStream {
    body: axum::body::BodyDataStream,
    buffered: String,
}

impl TimelineStream {
    async fn open(app: &Router, bearer: &str) -> Self {
        Self::open_at(app, bearer, "/console/timeline/stream").await
    }

    /// The stream at `uri` (a scoped stream names its identity there).
    async fn open_at(app: &Router, bearer: &str, uri: &str) -> Self {
        use tower::ServiceExt;
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(axum::body::Body::empty())
                    .expect("stream request"),
            )
            .await
            .expect("stream response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = Self {
            body: response.into_body().into_data_stream(),
            buffered: String::new(),
        };
        // The stream is live once its replay snapshot completed.
        let opened = stream.events_for(Duration::from_secs(5)).await;
        assert!(
            opened
                .iter()
                .any(|event| event.contains("snapshot_complete")),
            "the stream completes its snapshot: {opened:?}"
        );
        stream
    }

    /// The raw events read within `wait`, or until the stream ends.
    async fn events_for(&mut self, wait: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut events = Vec::new();
        loop {
            while let Some(end) = self.buffered.find("\n\n") {
                let event: String = self.buffered.drain(..end + 2).collect();
                events.push(event);
                if events
                    .last()
                    .is_some_and(|event| event.contains("snapshot_complete"))
                {
                    return events;
                }
            }
            match tokio::time::timeout_at(deadline, self.body.next()).await {
                Err(_) | Ok(None) => return events,
                Ok(Some(chunk)) => self
                    .buffered
                    .push_str(&String::from_utf8_lossy(&chunk.expect("a stream chunk"))),
            }
        }
    }
}

/// A metadata store that counts its reads of attention documents.
#[derive(Default)]
struct CountingMetadataStore {
    inner: crate::InMemoryMetadataStore,
    attention_reads: AtomicUsize,
}

impl CountingMetadataStore {
    /// The attention documents read since the last call.
    fn take_attention_reads(&self) -> usize {
        self.attention_reads.swap(0, Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl PersistentMetadataStore for CountingMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        self.inner.get_subscription_cursor(mob_id).await
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_subscription_cursor(mob_id, cursor).await
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<crate::MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        self.inner.load_member_idle_retire_overrides().await
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &crate::MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_member_idle_retire_override(record).await
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &crate::MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        self.inner.clear_member_idle_retire_override(record).await
    }

    async fn identity_tool_record(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<Option<crate::IdentityToolRecord>, MetadataStoreError> {
        self.inner.identity_tool_record(mob_id, identity).await
    }

    async fn set_identity_tool_record(
        &self,
        record: &crate::IdentityToolRecord,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_identity_tool_record(record).await
    }

    async fn load_identity_rotation_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<crate::IdentityRotationIntent>, MetadataStoreError> {
        self.inner.load_identity_rotation_intents(mob_id).await
    }

    async fn set_identity_rotation_intent(
        &self,
        intent: &crate::IdentityRotationIntent,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_identity_rotation_intent(intent).await
    }

    async fn clear_identity_rotation_intent(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<(), MetadataStoreError> {
        self.inner
            .clear_identity_rotation_intent(mob_id, identity)
            .await
    }

    async fn lifecycle_projection_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<crate::LifecycleProjectionCursor>, MetadataStoreError> {
        self.inner.lifecycle_projection_cursor(mob_id).await
    }

    async fn set_lifecycle_projection_cursor(
        &self,
        cursor: &crate::LifecycleProjectionCursor,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_lifecycle_projection_cursor(cursor).await
    }

    async fn lifecycle_effect_intent(
        &self,
        mob_id: &str,
        key: &crate::LifecycleIntentKey,
    ) -> Result<Option<crate::LifecycleEffectIntent>, MetadataStoreError> {
        self.inner.lifecycle_effect_intent(mob_id, key).await
    }

    async fn set_lifecycle_effect_intent(
        &self,
        intent: &crate::LifecycleEffectIntent,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_lifecycle_effect_intent(intent).await
    }

    async fn clear_lifecycle_effect_intent(
        &self,
        mob_id: &str,
        key: &crate::LifecycleIntentKey,
    ) -> Result<(), MetadataStoreError> {
        self.inner.clear_lifecycle_effect_intent(mob_id, key).await
    }

    async fn lifecycle_effect_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<crate::LifecycleEffectIntent>, MetadataStoreError> {
        self.inner.lifecycle_effect_intents(mob_id).await
    }

    async fn record_lifecycle_effect_failure(
        &self,
        failure: &crate::LifecycleEffectFailure,
    ) -> Result<(), MetadataStoreError> {
        self.inner.record_lifecycle_effect_failure(failure).await
    }

    async fn lifecycle_effect_failures(
        &self,
        mob_id: &str,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<crate::LifecycleEffectFailure>, MetadataStoreError> {
        self.inner
            .lifecycle_effect_failures(mob_id, skip, limit)
            .await
    }

    async fn prune_lifecycle_effect_failures(
        &self,
        mob_id: &str,
        keep: usize,
    ) -> Result<usize, MetadataStoreError> {
        self.inner
            .prune_lifecycle_effect_failures(mob_id, keep)
            .await
    }

    async fn settle_detached_writes(&self) {
        self.inner.settle_detached_writes().await;
    }

    fn realm_document_retention(&self) -> Option<crate::RecordRetention> {
        self.inner.realm_document_retention()
    }

    async fn realm_document(&self, key: &str) -> Result<Option<String>, MetadataStoreError> {
        if key.starts_with("console-attention/") {
            self.attention_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.realm_document(key).await
    }

    async fn set_realm_document(
        &self,
        key: &str,
        document: Option<&str>,
    ) -> Result<(), MetadataStoreError> {
        self.inner.set_realm_document(key, document).await
    }

    async fn realm_document_keys(&self, prefix: &str) -> Result<Vec<String>, MetadataStoreError> {
        self.inner.realm_document_keys(prefix).await
    }
}

/// A console log that counts its reads of send records.
#[derive(Default)]
struct CountingConsoleLog {
    inner: InMemoryConsoleLogStore,
    send_record_reads: AtomicUsize,
}

impl CountingConsoleLog {
    /// The send records read since the last call.
    fn take_send_record_reads(&self) -> usize {
        self.send_record_reads.swap(0, Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for CountingConsoleLog {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(frame_id, status).await
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
        self.inner.query_windowed_frames(query).await
    }

    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(dedupe_key).await
    }

    async fn send_frame_by_interaction(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.send_record_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.send_frame_by_interaction(interaction_id).await
    }

    async fn unsettled_send_frames(
        &self,
        window: usize,
        limit: usize,
    ) -> ConsoleLogResult<Vec<ConsoleFrame>> {
        self.inner.unsettled_send_frames(window, limit).await
    }

    async fn latest_activity_times(
        &self,
        identities: &[String],
    ) -> ConsoleLogResult<std::collections::HashMap<String, u64>> {
        self.inner.latest_activity_times(identities).await
    }

    fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        self.inner.send_dedupe_durability()
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        self.inner.history_prefix_revision().await
    }

    async fn compact_message_deltas(
        &self,
        compaction: &ConsoleDeltaCompaction,
    ) -> ConsoleLogResult<usize> {
        self.inner.compact_message_deltas(compaction).await
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime_key, source_kind, source_cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime_key, source_kind).await
    }
}

/// A person's console send records its thread: the member it went to, its
/// interaction, the audience of its replies and the person they are for
/// (the sender's private-owner key, as `whoami` reports it, the sender's
/// console principal, and the qualified principal the send carried to the
/// agent's run as its requester). A send to a shared agent is a shared
/// thread, a send to the sender's own private side chat a person thread.
/// An open console's anonymous send names no person to reply to.
#[tokio::test]
async fn thread_ref_recorded_with_requester() -> TestResult {
    let fixture = Fixture::enforced("mu16-thread-ref").await?;
    let alice = alice();
    let alice_scope = caller_scope(&fixture.app, &alice).await;
    let alice_requester = crate::runtime::console_human_principal(ISSUER, "oidc-sub-alice")?;
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let shared = send(&fixture.app, Some(&alice), "alpha", "thread-shared").await;
    let private = send(&fixture.app, Some(&alice), "alpha-side", "thread-private").await;
    let replies = fixture.replies();

    let reference = replies.thread_reference(&interaction_of(&shared)).await?;
    assert_eq!(
        reference,
        Some(ConsoleThreadReference {
            identity: "alpha".to_string(),
            interaction_id: interaction_of(&shared),
            input_frame_id: shared["input_frame_id"]
                .as_str()
                .expect("frame")
                .to_string(),
            audience: ConsoleThreadAudience::Shared,
            addressee: Some(alice_scope.clone()),
            access_principal: Some(ALICE.to_string()),
            requester: Some(alice_requester.clone()),
        }),
        "a person's send to a shared agent records a shared thread addressed to them"
    );
    let reference = replies.thread_reference(&interaction_of(&private)).await?;
    assert_eq!(
        reference,
        Some(ConsoleThreadReference {
            identity: "alpha-side".to_string(),
            interaction_id: interaction_of(&private),
            input_frame_id: private["input_frame_id"]
                .as_str()
                .expect("frame")
                .to_string(),
            audience: ConsoleThreadAudience::Person,
            addressee: Some(alice_scope),
            access_principal: Some(ALICE.to_string()),
            requester: Some(alice_requester),
        }),
        "a send to the sender's own private side chat records a person thread"
    );
    fixture.stop().await;

    let open = Fixture::new("mu16-thread-ref-open", false, None).await?;
    let anonymous = send(&open.app, None, "alpha", "thread-anonymous").await;
    let reference = open
        .replies()
        .thread_reference(&interaction_of(&anonymous))
        .await?;
    assert_eq!(
        reference.as_ref().map(|reference| (
            reference.audience,
            reference.addressee.clone(),
            reference.requester.clone()
        )),
        Some((ConsoleThreadAudience::Shared, None, None)),
        "an open console's anonymous send records a thread that names no person: {reference:?}"
    );
    open.stop().await;
    Ok(())
}

/// A reply to Alice's request in a shared agent is an item of Alice's
/// attention list, with its text and a link to her request, and of no one
/// else's: Bob and the access admin read none, and no timeline stream, Bob's
/// or Alice's, carries it.
#[tokio::test]
async fn person_addressed_item_visible_only_to_addressee() -> TestResult {
    let fixture = Fixture::enforced("mu16-item-addressee").await?;
    let (alice, bob, root) = (alice(), bob(), root());
    let sent = send(&fixture.app, Some(&alice), "alpha", "item-question").await;
    let mut bob_stream = TimelineStream::open(&fixture.app, &bob).await;
    let mut alice_stream = TimelineStream::open(&fixture.app, &alice).await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(
        is_attention(&receipt),
        "the reply to Alice's request in a shared agent is delivered as an item addressed to \
         her: {receipt:?}"
    );

    let capabilities =
        console_rpc_as(&fixture.app, Some(&alice), "meta/capabilities", json!({})).await;
    let methods = capabilities["result"]["methods"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        methods.contains(&json!("meta/console/attention"))
            && methods.contains(&json!("meta/console/attention/dismiss")),
        "the console lists its attention methods: {methods:?}"
    );
    let items = attention(&fixture.app, &alice).await;
    let items = items.expect("Alice reads her attention list");
    assert_eq!(items.len(), 1, "Alice's list holds the reply: {items:#?}");
    assert_eq!(items[0]["kind"], json!("private_reply"));
    assert_eq!(items[0]["text"], json!(REPLY_TEXT));
    assert_eq!(
        items[0]["origin"],
        json!({
            "identity": "alpha",
            "interaction_id": interaction_of(&sent),
            "input_frame_id": sent["input_frame_id"],
        }),
        "the item links to the request it answers"
    );
    for (who, bearer) in [("Bob", &bob), ("the access admin", &root)] {
        let items = attention(&fixture.app, bearer).await;
        assert_eq!(
            items.as_ref().map(Vec::len),
            Ok(0),
            "{who} reads no item addressed to Alice: {items:?}"
        );
    }
    for (who, stream) in [("Bob", &mut bob_stream), ("Alice", &mut alice_stream)] {
        let events = stream.events_for(Duration::from_secs(2)).await;
        assert!(
            !events.iter().any(|event| event.contains(REPLY_TEXT)),
            "{who}'s timeline stream carries no reply: {events:?}"
        );
    }
    fixture.stop().await;
    Ok(())
}

/// A reply to a request made in a shared agent is never appended to that
/// agent's transcript: no frame of it is there for Alice, Bob or the access
/// admin, and no frame anywhere carries it.
#[tokio::test]
async fn private_reply_never_appends_to_shared_transcript() -> TestResult {
    let fixture = Fixture::enforced("mu16-no-shared-frame").await?;
    let (alice, bob, root) = (alice(), bob(), root());
    let sent = send(&fixture.app, Some(&alice), "alpha", "shared-question").await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(
        is_attention(&receipt),
        "the reply goes to a person-addressed item: {receipt:?}"
    );
    for (who, bearer) in [
        ("Alice", &alice),
        ("Bob", &bob),
        ("the access admin", &root),
    ] {
        let frames = timeline(&fixture.app, bearer, Some("alpha"))
            .await
            .expect("the shared agent's timeline");
        assert!(
            frames
                .iter()
                .any(|frame| frame["interaction_id"] == json!(interaction_of(&sent))),
            "{who} reads Alice's question in the shared agent"
        );
        assert!(
            !frames
                .iter()
                .any(|frame| frame["kind"] == json!("private_reply") || carries_reply(frame)),
            "no reply frame is in the shared agent's transcript for {who}"
        );
    }
    let everything = timeline(&fixture.app, &root, None)
        .await
        .expect("the whole timeline");
    assert!(
        !everything.iter().any(carries_reply),
        "no frame anywhere carries the reply"
    );
    fixture.stop().await;
    Ok(())
}

/// A reply to Alice's request in her own private side chat is appended to
/// that side chat as a `private_reply` frame that answers her request: Alice
/// reads it there, Bob cannot read the side chat at all and no frame he
/// reads carries it, and no attention item is made.
#[tokio::test]
async fn private_reply_to_person_thread_appends_there() -> TestResult {
    let fixture = Fixture::enforced("mu16-person-thread").await?;
    let (alice, bob) = (alice(), bob());
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(&fixture.app, Some(&alice), "alpha-side", "side-question").await;

    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    let Ok(receipt) = receipt else {
        panic!("the reply to Alice's request in her side chat is delivered: {receipt:?}");
    };
    let ConsoleReplyDelivery::Thread { frame_id } = &receipt.delivery else {
        panic!("the reply is appended to her private thread: {receipt:?}");
    };

    let frames = timeline(&fixture.app, &alice, Some("alpha-side"))
        .await
        .expect("Alice reads her side chat");
    let reply = frames
        .iter()
        .find(|frame| frame["kind"] == json!("private_reply"))
        .unwrap_or_else(|| panic!("Alice's side chat holds the reply: {frames:#?}"));
    assert_eq!(reply["id"], json!(frame_id));
    assert_eq!(reply["payload"]["text"], json!(REPLY_TEXT));
    assert_eq!(
        reply["caused_by_frame_id"], sent["input_frame_id"],
        "the reply answers her request"
    );

    let hidden = timeline(&fixture.app, &bob, Some("alpha-side")).await;
    assert_eq!(
        hidden.as_ref().err().map(|error| error["code"].clone()),
        Some(json!(-32001)),
        "Bob cannot read Alice's side chat: {hidden:?}"
    );
    let bobs = timeline(&fixture.app, &bob, None)
        .await
        .expect("Bob's timeline");
    assert!(
        !bobs.iter().any(carries_reply),
        "no frame Bob reads carries the reply"
    );
    let items = attention(&fixture.app, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(0),
        "a reply in her own thread makes no attention item: {items:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// One reply is delivered once: the same reply id with the same text answers
/// the first delivery's receipt, other text is refused, and the receipt is
/// answered afterwards, which a host's committed check reads.
#[tokio::test]
async fn a_private_reply_is_delivered_once_per_reply_id() -> TestResult {
    let fixture = Fixture::enforced("mu16-once").await?;
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "once-question").await;
    let interaction = interaction_of(&sent);
    let replies = fixture.replies();

    let first = replies.deliver(&interaction, "reply-1", REPLY_TEXT).await;
    let Ok(first) = first else {
        panic!("the reply is delivered: {first:?}");
    };
    assert_eq!(
        replies.deliver(&interaction, "reply-1", REPLY_TEXT).await,
        Ok(first.clone()),
        "the same reply again answers the first delivery's receipt"
    );
    assert_eq!(
        replies
            .deliver(&interaction, "reply-1", "Another answer.")
            .await,
        Err(ConsoleReplyError::Conflict),
        "the reply id with other text is refused"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-1").await,
        Ok(Some(first)),
        "the delivery is answered afterwards"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-2").await,
        Ok(None),
        "a reply never delivered is not"
    );
    let items = attention(&fixture.app, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(1),
        "Alice holds one item: {items:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A host's payload policy may redact a private reply's frame before it is
/// stored. The reply is still delivered once into the person's thread: the
/// first delivery answers its receipt, the same reply again answers it too,
/// and the text the stored frame no longer carries is not read as other
/// text.
#[tokio::test]
async fn a_redacted_private_reply_is_delivered_once() -> TestResult {
    struct RedactReplies;
    impl ConsoleVisibilityPolicy for RedactReplies {
        fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
            (frame.kind == "private_reply").then(|| json!({ "redacted": true }))
        }
    }
    let fixture = Fixture::with_host_policy(
        "mu16-redacted-reply",
        true,
        Some(everyone_drives_every_agent()),
        Some(Arc::new(RedactReplies)),
    )
    .await?;
    let alice = alice();
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(
        &fixture.app,
        Some(&alice),
        "alpha-side",
        "redacted-question",
    )
    .await;
    let interaction = interaction_of(&sent);
    let replies = fixture.replies();

    let first = replies.deliver(&interaction, "reply-1", REPLY_TEXT).await;
    let Ok(first) = first else {
        panic!("the redacted reply is delivered: {first:?}");
    };
    assert!(
        matches!(first.delivery, ConsoleReplyDelivery::Thread { .. }),
        "the reply is appended to her private thread: {first:?}"
    );
    assert_eq!(
        replies.deliver(&interaction, "reply-1", REPLY_TEXT).await,
        Ok(first.clone()),
        "the same reply again answers the first delivery's receipt"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-1").await,
        Ok(Some(first)),
        "the delivery is answered afterwards"
    );
    fixture.stop().await;
    Ok(())
}

/// A host's payload policy that rewrites a private reply's frame rewrites
/// the attention item the reply becomes too: the item is stored and served
/// in the form the reply's frame would be stored in, so the person never
/// reads more in their attention list than in a thread.
#[tokio::test]
async fn a_redaction_policy_rewrites_the_attention_item_like_the_frame() -> TestResult {
    let fixture = Fixture::with_host_policy(
        "mu16-redacted-item",
        true,
        Some(everyone_drives_every_agent()),
        Some(Arc::new(WithholdReplies)),
    )
    .await?;
    let alice = alice();
    let alice_scope = caller_scope(&fixture.app, &alice).await;
    let sent = send(
        &fixture.app,
        Some(&alice),
        "alpha",
        "redacted-item-question",
    )
    .await;
    let interaction = interaction_of(&sent);
    let replies = fixture.replies();

    let first = replies.deliver(&interaction, "reply-1", REPLY_TEXT).await;
    let Ok(first) = first else {
        panic!("the reply is delivered: {first:?}");
    };
    let ConsoleReplyDelivery::Attention { item_id } = &first.delivery else {
        panic!("a reply to a request in a shared agent is an attention item: {first:?}");
    };

    let answer = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/console/attention",
        json!({}),
    )
    .await;
    assert!(
        !carries_reply(&answer),
        "the attention list serves none of the text the policy withheld: {answer:#}"
    );
    let items = answer["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(items.len(), 1, "{answer:#}");
    assert_eq!(items[0]["text"], json!(WITHHELD), "{answer:#}");
    assert_eq!(items[0]["redacted"], json!(true), "{answer:#}");
    let stored = fixture
        .owner
        .console_attention()
        .expect("the composition keeps an attention list")
        .item(&alice_scope, item_id)
        .await?;
    assert!(
        !format!("{stored:?}").contains(REPLY_TEXT),
        "the item is stored redacted: {stored:?}"
    );

    assert_eq!(
        replies.deliver(&interaction, "reply-1", REPLY_TEXT).await,
        Ok(first.clone()),
        "the same reply again answers the first delivery's receipt"
    );
    assert_eq!(
        replies.delivered(&interaction, "reply-1").await,
        Ok(Some(first)),
        "the delivery is answered afterwards"
    );
    fixture.stop().await;
    Ok(())
}

/// A console router whose own payload policy withholds private replies
/// serves the reply in the person's private thread withheld, and the
/// person's attention item no more fully: the list applies the serving
/// console's policy over the host's, as the thread does. A router that
/// keeps every payload serves the same item whole.
#[tokio::test]
async fn a_view_policy_withholds_the_item_like_the_thread() -> TestResult {
    let fixture = Fixture::with_host_policy(
        "mu16-view-policy",
        true,
        Some(everyone_drives_every_agent()),
        Some(Arc::new(AllowAllConsoleVisibilityPolicy)),
    )
    .await?;
    let app = fixture.router(decisions(true, false), Arc::new(WithholdReplies));
    let alice = alice();
    fork_side_chat(&app, &alice, "alpha", "alpha-side").await;
    let private = send(&app, Some(&alice), "alpha-side", "view-private").await;
    let shared = send(&app, Some(&alice), "alpha", "view-shared").await;
    let replies = fixture.replies();
    let in_thread = replies
        .deliver(&interaction_of(&private), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_thread(&in_thread), "{in_thread:?}");
    let as_item = replies
        .deliver(&interaction_of(&shared), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&as_item), "{as_item:?}");

    let frames = timeline(&app, &alice, Some("alpha-side"))
        .await
        .expect("Alice reads her side chat");
    let reply = frames
        .iter()
        .find(|frame| frame["kind"] == json!("private_reply"))
        .unwrap_or_else(|| panic!("the side chat holds the reply: {frames:#?}"));
    assert!(
        !carries_reply(reply),
        "the router's policy withholds the reply in the thread: {reply:#}"
    );
    let items = attention(&app, &alice).await.expect("Alice's list");
    assert_eq!(items.len(), 1, "{items:#?}");
    assert!(
        !items.iter().any(carries_reply),
        "the attention list shows no more of the reply than the thread: {items:#?}"
    );
    assert_eq!(items[0]["text"], json!(WITHHELD), "{items:#?}");
    assert_eq!(items[0]["redacted"], json!(true), "{items:#?}");

    let whole = attention(&fixture.app, &alice).await.expect("Alice's list");
    assert_eq!(
        whole.first().map(|item| item["text"].clone()),
        Some(json!(REPLY_TEXT)),
        "a router that keeps every payload serves the item whole: {whole:#?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A later policy change reaches the items delivered before it: replies
/// delivered while the console kept every payload, then the composition
/// restarts with a console whose policy withholds private replies. The
/// thread serves the earlier reply withheld, and the earlier attention item
/// no more fully.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_policy_withholds_the_earlier_item_like_the_thread() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mob_id = "mu16-later-policy";
    let alice = alice();
    {
        let first = composition(
            dir.path(),
            mob_id,
            Some(everyone_drives_every_agent()),
            |builder| builder,
        )
        .await?;
        seeded_member(first.mob_runtime(), "worker", "alpha", &[]).await?;
        let app = first.build_console_json_router_with_policy(
            decisions(true, false),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        fork_side_chat(&app, &alice, "alpha", "alpha-side").await;
        let private = send(&app, Some(&alice), "alpha-side", "later-private").await;
        let shared = send(&app, Some(&alice), "alpha", "later-shared").await;
        let replies = first.console_replies().expect("replies");
        let in_thread = replies
            .deliver(&interaction_of(&private), "reply-1", REPLY_TEXT)
            .await;
        assert!(is_thread(&in_thread), "{in_thread:?}");
        let as_item = replies
            .deliver(&interaction_of(&shared), "reply-1", REPLY_TEXT)
            .await;
        assert!(is_attention(&as_item), "{as_item:?}");
        let items = attention(&app, &alice).await.expect("Alice's list");
        assert!(items.iter().any(carries_reply), "{items:#?}");
        let _ = first.shutdown().await;
    }
    let second = composition(
        dir.path(),
        mob_id,
        Some(everyone_drives_every_agent()),
        |builder| builder,
    )
    .await?;
    let app = second
        .build_console_json_router_with_policy(decisions(true, false), Arc::new(WithholdReplies));
    let frames = timeline(&app, &alice, Some("alpha-side"))
        .await
        .expect("Alice reads her side chat after the restart");
    let reply = frames
        .iter()
        .find(|frame| frame["kind"] == json!("private_reply"))
        .cloned()
        .unwrap_or_else(|| panic!("the side chat still holds the reply: {frames:#?}"));
    assert!(
        !carries_reply(&reply),
        "the new policy withholds the earlier reply in the thread: {reply:#}"
    );
    let items = attention(&app, &alice).await.expect("Alice's list");
    assert_eq!(items.len(), 1, "{items:#?}");
    assert!(
        !items.iter().any(carries_reply),
        "the attention list shows no more of the earlier reply than the thread: {items:#?}"
    );
    let _ = second.shutdown().await;
    Ok(())
}

/// An attention item is listed only while its person may still view the
/// agent the reply came from, as that agent's own frames are: once the
/// access configuration no longer lets Alice view `alpha`, her item from it
/// is left out of her list. The item is kept: access granted again lists it
/// again.
#[tokio::test]
async fn an_item_is_listed_only_while_its_person_may_view_the_agent() -> TestResult {
    let mut fixture = Fixture::enforced("mu16-access-revoked").await?;
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "revoked-question").await;
    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&receipt), "{receipt:?}");
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1),
        "Alice reads her item while she may view alpha"
    );

    fixture
        .owner
        .set_access_controller(no_one_drives_any_agent());
    let revoked = fixture.router(
        decisions(true, false),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let frames = timeline(&revoked, &alice, Some("alpha")).await;
    assert!(
        !matches!(
            &frames,
            Ok(frames) if frames
                .iter()
                .any(|frame| frame["interaction_id"] == json!(interaction_of(&sent)))
        ),
        "Alice no longer reads alpha's frames: {frames:?}"
    );
    let items = attention(&revoked, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(0),
        "her item from alpha is left out of her list: {items:?}"
    );

    fixture
        .owner
        .set_access_controller(everyone_drives_every_agent());
    let granted = fixture.router(
        decisions(true, false),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let items = attention(&granted, &alice).await;
    assert_eq!(
        items.as_ref().map(Vec::len),
        Ok(1),
        "access granted again lists the item again: {items:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// One item this build cannot read (a later format, or a damaged document)
/// is left out of its person's list, and the rest of the list is still
/// read.
#[tokio::test]
async fn one_unreadable_item_keeps_the_rest_of_the_list() -> TestResult {
    let fixture = Fixture::enforced("mu16-unreadable").await?;
    let alice = alice();
    let alice_scope = caller_scope(&fixture.app, &alice).await;
    let sent = send(&fixture.app, Some(&alice), "alpha", "unreadable-question").await;
    let replies = fixture.replies();
    let later = replies
        .deliver(
            &interaction_of(&sent),
            "reply-later",
            "A later build's reply.",
        )
        .await;
    let Ok(ConsoleReplyReceipt {
        delivery: ConsoleReplyDelivery::Attention { item_id: later_id },
        ..
    }) = later
    else {
        panic!("the first reply is an item: {later:?}");
    };
    let readable = replies
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&readable), "{readable:?}");

    // A later build rewrites the first item in a format this one cannot
    // read: every attention document that names it.
    let store = fixture.owner.persistent_metadata();
    let later_format = json!({
        "v": 2,
        "addressee": alice_scope,
        "item": { "id": later_id, "created_at_ms": 1, "kind": "a_later_kind" },
    })
    .to_string();
    let mut rewritten = 0;
    for key in store
        .realm_document_keys("console-attention/")
        .await
        .map_err(|error| error.to_string())?
    {
        if key.contains(&later_id) {
            store
                .set_realm_document(&key, Some(&later_format))
                .await
                .map_err(|error| error.to_string())?;
            rewritten += 1;
        }
    }
    assert!(rewritten > 0, "the item's documents are rewritten");

    let read = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/console/attention",
        json!({}),
    )
    .await;
    assert!(
        read["error"].is_null(),
        "one unreadable item does not fail the list: {read:#}"
    );
    let items = read["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(items.len(), 1, "{read:#}");
    assert_eq!(
        items[0]["text"],
        json!(REPLY_TEXT),
        "the readable item is still listed: {read:#}"
    );
    fixture.stop().await;
    Ok(())
}

/// The attention list reads only the items not dismissed: once Alice
/// dismissed two of her three items, a read of her list reads the one left
/// and no dismissed one.
#[tokio::test]
async fn the_attention_list_reads_no_dismissed_item() -> TestResult {
    let store = Arc::new(CountingMetadataStore::default());
    let fixture = Fixture::build(
        "mu16-dismissed-reads",
        true,
        Some(everyone_drives_every_agent()),
        None,
        {
            let store = Arc::clone(&store);
            move |builder| builder.persistent_metadata(store)
        },
    )
    .await?;
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "dismissed-reads").await;
    let replies = fixture.replies();
    for reply in ["reply-1", "reply-2", "reply-3"] {
        let delivered = replies
            .deliver(
                &interaction_of(&sent),
                reply,
                &format!("{REPLY_TEXT} {reply}"),
            )
            .await;
        assert!(is_attention(&delivered), "{delivered:?}");
    }
    let items = attention(&fixture.app, &alice).await.expect("Alice's list");
    assert_eq!(items.len(), 3, "{items:#?}");
    for item in &items[1..] {
        let dismissed = console_rpc_as(
            &fixture.app,
            Some(&alice),
            "meta/console/attention/dismiss",
            json!({ "item_id": item["id"] }),
        )
        .await;
        assert_eq!(
            dismissed["result"]["dismissed"],
            json!(true),
            "{dismissed:#}"
        );
    }

    store.take_attention_reads();
    let items = attention(&fixture.app, &alice).await.expect("Alice's list");
    assert_eq!(items.len(), 1, "{items:#?}");
    assert_eq!(
        store.take_attention_reads(),
        1,
        "the read reads the one item not dismissed, and no dismissed one"
    );
    fixture.stop().await;
    Ok(())
}

/// One attention read reads a bounded number of items however many its
/// person holds: at most twice the page (the newest not dismissed), and it
/// answers at most the page, newest first.
#[tokio::test]
async fn an_attention_read_is_bounded() -> TestResult {
    let store = Arc::new(CountingMetadataStore::default());
    let fixture = Fixture::build(
        "mu16-bounded-read",
        true,
        Some(everyone_drives_every_agent()),
        None,
        {
            let store = Arc::clone(&store);
            move |builder| builder.persistent_metadata(store)
        },
    )
    .await?;
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "bounded-read").await;
    let replies = fixture.replies();
    let held = 2 * CONSOLE_ATTENTION_PAGE + 10;
    for reply in 0..held {
        let delivered = replies
            .deliver(
                &interaction_of(&sent),
                &format!("reply-{reply}"),
                &format!("Reply {reply}."),
            )
            .await;
        assert!(is_attention(&delivered), "{delivered:?}");
    }

    store.take_attention_reads();
    let items = attention(&fixture.app, &alice).await.expect("Alice's list");
    let reads = store.take_attention_reads();
    assert_eq!(
        items.len(),
        CONSOLE_ATTENTION_PAGE,
        "the read answers a page"
    );
    assert!(
        reads <= 2 * CONSOLE_ATTENTION_PAGE,
        "the read reads at most twice the page, not all {held} items: {reads} reads"
    );
    let created: Vec<u64> = items
        .iter()
        .map(|item| item["created_at_ms"].as_u64().unwrap_or_default())
        .collect();
    assert!(
        created.windows(2).all(|pair| pair[0] >= pair[1]),
        "newest first: {created:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// A delivery reads its request's send record once, whether the reply goes
/// to the person's thread or to their attention list: the record is found
/// by a scan of the console log, which no index shortens.
#[tokio::test]
async fn a_delivery_reads_its_send_record_once() -> TestResult {
    let log = Arc::new(CountingConsoleLog::default());
    let fixture = Fixture::build(
        "mu16-one-lookup",
        true,
        Some(everyone_drives_every_agent()),
        None,
        {
            let log = Arc::clone(&log);
            move |builder| builder.with_console_log_store(log)
        },
    )
    .await?;
    let alice = alice();
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let shared = send(&fixture.app, Some(&alice), "alpha", "lookup-shared").await;
    let private = send(&fixture.app, Some(&alice), "alpha-side", "lookup-private").await;
    let replies = fixture.replies();

    log.take_send_record_reads();
    let as_item = replies
        .deliver(&interaction_of(&shared), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&as_item), "{as_item:?}");
    assert_eq!(
        log.take_send_record_reads(),
        1,
        "a delivery to the attention list reads the send record once"
    );
    let in_thread = replies
        .deliver(&interaction_of(&private), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_thread(&in_thread), "{in_thread:?}");
    assert_eq!(
        log.take_send_record_reads(),
        1,
        "a delivery to the person's thread reads the send record once"
    );
    fixture.stop().await;
    Ok(())
}

/// A request whose sender names no person has no private reply route: a
/// service's send on a console with sign-in and an anonymous send on an open
/// console are refused `NoAddressee`, an interaction no send recorded is
/// `UnknownThread`, and the route says it cannot deliver either.
#[tokio::test]
async fn a_request_without_an_addressee_has_no_private_reply_route() -> TestResult {
    let fixture = Fixture::enforced("mu16-no-addressee").await?;
    let sent = send(&fixture.app, Some(&robot()), "alpha", "robot-question").await;
    let replies = fixture.replies();
    assert_eq!(
        replies.can_deliver(&interaction_of(&sent)).await,
        Ok(false),
        "a service's request has no private reply route"
    );
    assert_eq!(
        replies
            .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
            .await,
        Err(ConsoleReplyError::NoAddressee),
        "a reply to a service's request is refused"
    );
    assert_eq!(
        replies
            .deliver(
                "00000000-0000-5000-8000-000000000000",
                "reply-1",
                REPLY_TEXT
            )
            .await,
        Err(ConsoleReplyError::UnknownThread),
        "a reply to an interaction no send recorded is refused"
    );
    fixture.stop().await;

    let open = Fixture::new("mu16-no-addressee-open", false, None).await?;
    let anonymous = send(&open.app, None, "alpha", "anonymous-question").await;
    let replies = open.replies();
    assert_eq!(
        replies.can_deliver(&interaction_of(&anonymous)).await,
        Ok(false)
    );
    assert_eq!(
        replies
            .deliver(&interaction_of(&anonymous), "reply-1", REPLY_TEXT)
            .await,
        Err(ConsoleReplyError::NoAddressee),
        "a reply to an open console's anonymous request is refused"
    );
    open.stop().await;
    Ok(())
}

/// Only the addressee dismisses an attention item: Bob's dismiss of Alice's
/// item dismisses nothing, Alice's hides it from her list, and the reply
/// stays delivered.
#[tokio::test]
async fn only_the_addressee_dismisses_an_attention_item() -> TestResult {
    let fixture = Fixture::enforced("mu16-dismiss").await?;
    let (alice, bob) = (alice(), bob());
    let sent = send(&fixture.app, Some(&alice), "alpha", "dismiss-question").await;
    let replies = fixture.replies();
    let delivered = replies
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(delivered.is_ok(), "the reply is delivered: {delivered:?}");
    let items = attention(&fixture.app, &alice)
        .await
        .expect("Alice's attention list");
    let item_id = items
        .first()
        .and_then(|item| item["id"].as_str())
        .unwrap_or_else(|| panic!("Alice holds the item: {items:#?}"))
        .to_string();

    let by_bob = console_rpc_as(
        &fixture.app,
        Some(&bob),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id }),
    )
    .await;
    assert_eq!(
        by_bob["result"],
        json!({ "item_id": item_id, "dismissed": false }),
        "Bob dismisses none of Alice's items: {by_bob:#}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1),
        "Alice still holds it"
    );
    let by_alice = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id }),
    )
    .await;
    assert_eq!(
        by_alice["result"],
        json!({ "item_id": item_id, "dismissed": true }),
        "{by_alice:#}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(0),
        "a dismissed item is no longer listed"
    );
    assert!(
        matches!(
            replies.delivered(&interaction_of(&sent), "reply-1").await,
            Ok(Some(_))
        ),
        "the reply stays delivered"
    );
    fixture.stop().await;
    Ok(())
}

/// No param names whose attention items are read, and a dismissal made for
/// another caller is refused: Bob passes Alice's caller scope, principal or
/// agent under every plausible name and reads nothing of hers, and Alice's
/// dismissal that carries Bob's caller scope is refused (`caller_changed`),
/// her item still listed.
#[tokio::test]
async fn no_param_reads_or_dismisses_another_persons_items() -> TestResult {
    let fixture = Fixture::enforced("mu16-params").await?;
    let (alice, bob) = (alice(), bob());
    let alice_scope = caller_scope(&fixture.app, &alice).await;
    let bob_scope = caller_scope(&fixture.app, &bob).await;
    let sent = send(&fixture.app, Some(&alice), "alpha", "params-question").await;
    let delivered = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&delivered), "{delivered:?}");
    for params in [
        json!({ "addressee": alice_scope }),
        json!({ "caller_scope": alice_scope }),
        json!({ "expected_caller_scope": alice_scope }),
        json!({ "private_owner_key": alice_scope }),
        json!({ "principal": ALICE }),
        json!({ "identity": "alpha" }),
    ] {
        let read = console_rpc_as(
            &fixture.app,
            Some(&bob),
            "meta/console/attention",
            params.clone(),
        )
        .await;
        assert!(
            !carries_reply(&read),
            "Bob reads none of Alice's items with {params}: {read:#}"
        );
    }
    let items = attention(&fixture.app, &alice).await.expect("Alice's list");
    let item_id = items[0]["id"].as_str().expect("an item id").to_string();
    let refused = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id, "expected_caller_scope": bob_scope }),
    )
    .await;
    assert_eq!(
        refused["error"]["code"],
        json!(-32056),
        "a dismissal made for another caller is refused: {refused:#}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1),
        "Alice still holds the item"
    );
    fixture.stop().await;
    Ok(())
}

/// A read-only console lists the attention read without the dismissal, and
/// refuses a dismissal (`-32010`), the item still listed.
#[tokio::test]
async fn a_read_only_console_refuses_a_dismissal() -> TestResult {
    let fixture = Fixture::enforced("mu16-read-only").await?;
    let read_only = fixture.router(
        decisions(true, true),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let alice = alice();
    let sent = send(&fixture.app, Some(&alice), "alpha", "read-only-question").await;
    let delivered = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_attention(&delivered), "{delivered:?}");
    let capabilities =
        console_rpc_as(&read_only, Some(&alice), "meta/capabilities", json!({})).await;
    let methods = capabilities["result"]["methods"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        methods.contains(&json!("meta/console/attention")),
        "{methods:?}"
    );
    assert!(
        !methods.contains(&json!("meta/console/attention/dismiss")),
        "{methods:?}"
    );
    let items = attention(&read_only, &alice).await.expect("Alice's list");
    let item_id = items[0]["id"].as_str().expect("an item id").to_string();
    let refused = console_rpc_as(
        &read_only,
        Some(&alice),
        "meta/console/attention/dismiss",
        json!({ "item_id": item_id }),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32010), "{refused:#}");
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1),
        "the item is still listed"
    );
    fixture.stop().await;
    Ok(())
}

/// A reply to a request made in a private side chat that is gone when the
/// reply comes becomes the person's attention item, and no one else's.
#[tokio::test]
async fn a_reply_to_a_retired_side_chat_becomes_an_item() -> TestResult {
    let fixture = Fixture::enforced("mu16-thread-gone").await?;
    let (alice, bob) = (alice(), bob());
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(&fixture.app, Some(&alice), "alpha-side", "gone-question").await;
    let retired = console_rpc_as(
        &fixture.app,
        Some(&alice),
        "meta/retire",
        json!({ "identity": "alpha-side" }),
    )
    .await;
    assert!(retired["error"].is_null(), "{retired:#}");
    let gone = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let member = fixture
                .owner
                .mob_runtime()
                .handle()
                .get_member(&meerkat_mob::ids::AgentIdentity::from("alpha-side"))
                .await
                .expect("roster read");
            if member.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "the side chat leaves the roster");
    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(
        is_attention(&receipt),
        "the reply to a request in a side chat that is gone is an item: {receipt:?}"
    );
    assert_eq!(
        attention(&fixture.app, &alice)
            .await
            .map(|items| items.len()),
        Ok(1)
    );
    assert_eq!(
        attention(&fixture.app, &bob).await.map(|items| items.len()),
        Ok(0)
    );
    fixture.stop().await;
    Ok(())
}

/// A reply appended to a person's private thread reaches that person's
/// timeline stream and no other person's.
#[tokio::test]
async fn a_person_thread_reply_streams_to_its_person_only() -> TestResult {
    let fixture = Fixture::enforced("mu16-thread-stream").await?;
    let (alice, bob) = (alice(), bob());
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(&fixture.app, Some(&alice), "alpha-side", "stream-question").await;
    let mut bob_stream = TimelineStream::open(&fixture.app, &bob).await;
    // Alice's stream is scoped to her side chat: a global stream may miss
    // the first live frame of an identity its read model has not listed yet.
    let mut alice_stream = TimelineStream::open_at(
        &fixture.app,
        &alice,
        "/console/timeline/stream?identity=alpha-side",
    )
    .await;
    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_thread(&receipt), "{receipt:?}");
    let alice_events = alice_stream.events_for(Duration::from_secs(3)).await;
    let bob_events = bob_stream.events_for(Duration::from_secs(3)).await;
    assert!(
        alice_events.iter().any(|event| event.contains(REPLY_TEXT)),
        "Alice's stream carries her reply: {alice_events:?}"
    );
    assert!(
        !bob_events.iter().any(|event| event.contains(REPLY_TEXT)),
        "Bob's stream carries no reply: {bob_events:?}"
    );
    fixture.stop().await;
    Ok(())
}

/// The reply's frame carries the request frame's captured member context and
/// answers the request frame.
#[tokio::test]
async fn a_thread_reply_frame_carries_the_request_context() -> TestResult {
    let fixture = Fixture::enforced("mu16-thread-context").await?;
    let alice = alice();
    fork_side_chat(&fixture.app, &alice, "alpha", "alpha-side").await;
    let sent = send(&fixture.app, Some(&alice), "alpha-side", "context-question").await;
    let receipt = fixture
        .replies()
        .deliver(&interaction_of(&sent), "reply-1", REPLY_TEXT)
        .await;
    assert!(is_thread(&receipt), "{receipt:?}");
    let store = fixture.owner.console_log_store();
    let request = store
        .send_frame_by_interaction(&interaction_of(&sent))
        .await
        .map_err(|error| error.to_string())?
        .expect("the send record");
    let page = store
        .query_frames(ConsoleTimelineQuery {
            identity: Some("alpha-side".to_string()),
            limit: 500,
            ..ConsoleTimelineQuery::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    let reply = page
        .frames
        .iter()
        .find(|frame| frame.kind == "private_reply")
        .expect("the reply frame");
    assert!(request.source.member_provenance.is_some());
    assert_eq!(
        reply.source.member_provenance, request.source.member_provenance,
        "the reply carries the request's captured member context"
    );
    assert_eq!(
        reply.caused_by_frame_id.as_deref(),
        Some(request.id.as_str())
    );
    fixture.stop().await;
    Ok(())
}

/// A candidate launch refuses a dismissal, which writes the metadata store
/// the authoritative launch reads, and still answers the list.
#[test]
fn a_candidate_launch_refuses_an_attention_dismissal() {
    let refused = crate::rpc::CANDIDATE_LAUNCH_REFUSED_METHODS;
    assert!(refused.contains(&crate::rpc::CONSOLE_ATTENTION_DISMISS_METHOD));
    assert!(!refused.contains(&crate::rpc::CONSOLE_ATTENTION_METHOD));
}
