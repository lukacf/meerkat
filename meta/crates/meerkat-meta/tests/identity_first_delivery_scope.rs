#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Scope-bound internal dispatch through the identity runtime and the
//! gateway, on real persistent stores (meerkat's SQLite session and runtime
//! stores under `persistent_state`, an on-disk continuity store).
//!
//! A host captures an identity's delivery scope from `meta/status_identity`,
//! persists it, dispatches against it with `expected_scope`, and recovers a
//! lost reply from the scope's ORIGINAL session with
//! `meta/recover_delivery`. The scoped path never materializes, repairs,
//! retargets or prepares a delivery: a moved scope is refused typed with
//! nothing submitted.
//!
//! Every wait is a typed barrier: the model double signals each call on a
//! channel and holds its reply until released, and a delivery's terminal is
//! awaited through meerkat's exact per-delivery wait.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, LeaseProvider, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::RestoreOutcome;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeServices,
    ContinuityStore, CustomizerError, DeliveryScope, DispatchInput, DurabilityPolicy,
    DurableAgentSpec, IdentityFirstRuntimeContext, IdentityLifecycleState, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, ManagedPeerEdge,
    MobSessionBridge, MutableRosterProvider, ScopeMismatch, ScopedDeliveryError, ScopedRecovery,
    ScopedRetireError, SessionBridge, TopologyContext, TopologyError, TurnOutput,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const WAIT: Duration = Duration::from_secs(30);
const IDENTITY: &str = "personal:alice";
const OTHER: &str = "personal:bob";
const ANSWER: &str = "scoped-answer";

fn identity() -> AgentIdentity {
    AgentIdentity::parse(IDENTITY).unwrap()
}

fn spec() -> DurableAgentSpec {
    spec_for(IDENTITY)
}

fn spec_for(identity: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).unwrap(),
        profile: ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }
}

fn unique_mob_id() -> String {
    format!("delivery-scope-{}", uuid::Uuid::new_v4())
}

const MOB_TOML: &str = r#"
[mob]
id = "MOB_ID_PLACEHOLDER"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&MOB_TOML.replace("MOB_ID_PLACEHOLDER", mob_id))
        .expect("parse delivery-scope mob definition")
}

struct EmptyTopology;
#[async_trait]
impl TopologyProvider for EmptyTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(vec![])
    }
}

struct NoopCustomizer;
#[async_trait]
impl AgentCustomizer for NoopCustomizer {
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
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// Signals every model call on a channel and holds each reply until the test
/// releases the model, then answers [`ANSWER`].
#[derive(Clone)]
struct BarrierClient {
    calls: tokio::sync::mpsc::UnboundedSender<()>,
    released: tokio::sync::watch::Sender<bool>,
}

impl BarrierClient {
    fn new(released: bool) -> (Self, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let (calls, calls_rx) = tokio::sync::mpsc::unbounded_channel();
        let (released, _) = tokio::sync::watch::channel(released);
        (Self { calls, released }, calls_rx)
    }

    fn release(&self) {
        self.released.send_replace(true);
    }
}

impl meerkat_client::LlmClient for BarrierClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let _ = self.calls.send(());
        let mut released = self.released.subscribe();
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            while !*released.borrow_and_update() {
                if released.changed().await.is_err() {
                    break;
                }
            }
            yield Ok(LlmEvent::TextDelta { delta: ANSWER.to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }
    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }
    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// One boot of the gateway over `state_path`: the unified runtime, its
/// identity runtime and the RPC context, with the identity restored.
struct Boot {
    unified: meerkat_meta::UnifiedRuntime,
    identity_rt: Arc<IdentityRuntime>,
    /// Absent when the identity runtime was composed without the unified
    /// runtime's installation ([`Boot::start_standalone`]).
    ctx: Option<meerkat_meta::rpc::IdentityFirstContext>,
}

impl Boot {
    async fn start(state_path: &std::path::Path, mob_id: &str, client: &BarrierClient) -> Self {
        Self::start_with(state_path, mob_id, client, vec![spec()]).await
    }

    async fn start_with(
        state_path: &std::path::Path,
        mob_id: &str,
        client: &BarrierClient,
        specs: Vec<DurableAgentSpec>,
    ) -> Self {
        let definition = definition(mob_id);
        let mut unified = Self::unified(state_path, &definition, client).await;
        // 0.9 exports the composition's bridge and identity runtime only as
        // owning views, so the identity runtime under test drives its own
        // native bridge and is installed into the composition, as the
        // gateway installs its own.
        let identity_rt =
            Self::identity_runtime(&unified, state_path, Arc::new(LocalLeaseProvider::new()));
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            Arc::clone(&identity_rt),
            Arc::new(MutableRosterProvider::new(specs.clone())),
            Some(Arc::new(EmptyTopology) as Arc<dyn TopologyProvider>),
            Some(Arc::new(NoopCustomizer) as Arc<dyn AgentCustomizer>),
            Some(definition.clone()),
        ));
        let result = unified
            .install_and_bootstrap_identity_first_context(context, &specs)
            .await
            .expect("install and restore the identity-first runtime");
        for spec in &specs {
            assert!(
                matches!(
                    result.outcomes.get(&spec.identity),
                    Some(RestoreOutcome::Created { .. } | RestoreOutcome::Resumed { .. })
                ),
                "{:?}",
                result.outcomes
            );
        }
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: unified
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(specs)),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(definition),
            transcript_edit_service: None,
            compaction_floors: None,
        };
        Self {
            unified,
            identity_rt,
            ctx: Some(ctx),
        }
    }

    async fn unified(
        state_path: &std::path::Path,
        definition: &MobDefinition,
        client: &BarrierClient,
    ) -> meerkat_meta::UnifiedRuntime {
        UnifiedRuntimeBuilder::default()
            .definition(definition.clone())
            .persistent_state(state_path)
            .comms(true)
            .default_llm_client(Arc::new(client.clone()))
            .build()
            .await
            .expect("build UnifiedRuntime")
    }

    /// The identity runtime under test: its own native bridge over
    /// `unified`'s mob, an on-disk continuity store and `lease_provider`.
    fn identity_runtime(
        unified: &meerkat_meta::UnifiedRuntime,
        state_path: &std::path::Path,
        lease_provider: Arc<dyn LeaseProvider>,
    ) -> Arc<IdentityRuntime> {
        let bridge: Arc<dyn SessionBridge> = Arc::new(MobSessionBridge::with_session_service(
            unified.mob_handle(),
            unified
                .mob_runtime()
                .session_service()
                .cloned()
                .expect("the persistent runtime has a session service"),
        ));
        let store = Arc::new(
            LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
        );
        Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: store as Arc<dyn ContinuityStore>,
                lease_provider,
                runtime_instance_id: "delivery-scope".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
        )
    }

    /// One JSON-RPC call; the whole reply.
    async fn call(&self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let reply = meerkat_meta::rpc::handle_unified_rpc_json(
            &self.unified,
            &request.to_string(),
            WAIT,
            None,
            self.ctx.as_ref(),
        )
        .await;
        serde_json::from_str(&reply).expect("json-rpc reply")
    }

    /// One JSON-RPC call that must succeed; its result.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params).await;
        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    async fn status_scope(&self) -> Value {
        self.status_scope_of(IDENTITY).await
    }

    async fn status_scope_of(&self, identity: &str) -> Value {
        let status = self
            .rpc("meta/status_identity", json!({"identity": identity}))
            .await;
        assert!(status["delivery_scope"].is_object(), "{status}");
        status["delivery_scope"].clone()
    }

    async fn scoped_dispatch(
        &self,
        scope: &Value,
        text: &str,
        key: &str,
        correlation: &str,
    ) -> Value {
        self.scoped_dispatch_as(IDENTITY, scope, text, key, correlation)
            .await
    }

    async fn scoped_dispatch_as(
        &self,
        identity: &str,
        scope: &Value,
        text: &str,
        key: &str,
        correlation: &str,
    ) -> Value {
        self.call(
            "meta/dispatch",
            json!({
                "identity": identity,
                "dispatch_input": {
                    "content": text,
                    "idempotency_key": key,
                    "correlation_id": correlation,
                },
                "expected_scope": scope,
            }),
        )
        .await
    }

    async fn recover(&self, scope: &Value, key: &str, correlation: &str) -> Value {
        let reply = self.recover_reply(IDENTITY, scope, key, correlation).await;
        assert!(
            reply.get("error").is_none(),
            "meta/recover_delivery failed: {reply}"
        );
        reply["result"].clone()
    }

    /// One `meta/recover_delivery` call for `identity`; the whole reply.
    async fn recover_reply(
        &self,
        identity: &str,
        scope: &Value,
        key: &str,
        correlation: &str,
    ) -> Value {
        self.call(
            "meta/recover_delivery",
            json!({
                "identity": identity,
                "scope": scope,
                "idempotency_key": key,
                "correlation_id": correlation,
                "timeout_ms": 10_000,
            }),
        )
        .await
    }

    /// Wait for the delivery's exact terminal in the member's current
    /// session: a committed boundary, not a clock.
    async fn wait_terminal(&self, scope: &DeliveryScope, key: &str, correlation: &str) {
        let delivery = meerkat_mob::MobDeliveryIdentity::new(key, correlation).unwrap();
        let spec = meerkat_mob::BoundedResultSpec::new("delivery-scope", 4096).unwrap();
        self.unified
            .mob_handle()
            .wait_bounded_work_for_identity_with_delivery_identity(
                scope.member().agent_identity(),
                &delivery,
                &spec,
                std::time::Instant::now() + WAIT,
            )
            .await
            .expect("the delivery reaches its exact terminal");
    }

    async fn shutdown(self) {
        self.unified.shutdown().await;
    }
}

/// Assert no model call happened since the receiver was last drained.
fn assert_no_model_call(calls: &mut tokio::sync::mpsc::UnboundedReceiver<()>, why: &str) {
    assert!(
        matches!(
            calls.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "{why}"
    );
}

async fn next_model_call(calls: &mut tokio::sync::mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(WAIT, calls.recv())
        .await
        .expect("the model is called")
        .expect("model call channel open");
}

/// G1 + G9: capture is observational, the persisted scope and receipt
/// survive a full restart byte-exact, and recovery after the restart reads
/// the original input's own terminal result from the original session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persisted_scope_and_receipt_survive_restart_and_recover_the_original_result() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let mob_id = unique_mob_id();
    let (client, mut calls) = BarrierClient::new(false);
    let key = "calendar-occurrence-1";
    let correlation = uuid::Uuid::new_v4().to_string();

    let boot = Boot::start(&state_path, &mob_id, &client).await;
    // Capture twice through the gateway and once natively: observational,
    // identical, and no model call.
    let scope_json = boot.status_scope().await;
    assert_eq!(scope_json, boot.status_scope().await);
    let native = boot
        .identity_rt
        .capture_delivery_scope(&identity())
        .await
        .expect("native capture");
    assert_eq!(native.to_json_value(), scope_json);
    assert_eq!(scope_json["version"], 1, "{scope_json}");
    // Core's member scope names the mob incarnation it was captured at.
    assert_eq!(
        scope_json["member"]["version"],
        meerkat_mob::MEMBER_DELIVERY_SCOPE_VERSION,
        "{scope_json}"
    );
    assert!(
        scope_json["member"]["mob_incarnation"].is_string(),
        "{scope_json}"
    );
    let status = boot
        .rpc("meta/status_identity", json!({"identity": IDENTITY}))
        .await;
    assert_eq!(
        scope_json["member"]["session_id"], status["session_id"],
        "the scope pins the identity's current session"
    );
    assert_no_model_call(&mut calls, "capture submits nothing");

    // The host persists the scope before it dispatches.
    let persisted_scope = serde_json::to_string(&scope_json).unwrap();
    let scope = DeliveryScope::from_json_value(serde_json::from_str(&persisted_scope).unwrap())
        .expect("persisted scope decodes");

    let reply = boot
        .scoped_dispatch(&scope_json, "occurrence one", key, &correlation)
        .await;
    let result = reply["result"].clone();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(result["receipt"]["stage"], "ingress_accepted", "{result}");
    assert_eq!(
        result["receipt"]["session_id"], scope_json["member"]["session_id"],
        "{result}"
    );
    assert_eq!(result["delivery_scope"], scope_json, "{result}");
    let persisted_receipt = serde_json::to_string(&result["receipt"]).unwrap();
    next_model_call(&mut calls).await;
    // In flight while the model holds the turn: the original input, never a
    // claim of absence.
    let in_flight = boot.recover(&scope_json, key, &correlation).await;
    assert_eq!(in_flight["recovery"]["state"], "in_flight", "{in_flight}");
    client.release();
    boot.wait_terminal(&scope, key, &correlation).await;
    boot.shutdown().await;

    // A fresh process over the same stores: the persisted bytes decode to
    // the same scope and recover the original input's own result.
    let boot = Boot::start(&state_path, &mob_id, &client).await;
    let reloaded: Value = serde_json::from_str(&persisted_scope).unwrap();
    assert_eq!(
        DeliveryScope::from_json_value(reloaded.clone()).unwrap(),
        scope
    );
    let receipt: Value = serde_json::from_str(&persisted_receipt).unwrap();
    assert_eq!(receipt["session_id"], reloaded["member"]["session_id"]);
    let recovered = boot.recover(&reloaded, key, &correlation).await;
    assert_eq!(recovered["recovery"]["state"], "completed", "{recovered}");
    assert_eq!(
        recovered["recovery"]["output_status"], "text",
        "{recovered}"
    );
    assert_eq!(recovered["recovery"]["output"], ANSWER, "{recovered}");
    assert_eq!(recovered["delivery_scope"], reloaded, "{recovered}");
    assert_no_model_call(&mut calls, "recovery resubmits nothing");
    boot.shutdown().await;
}

/// G4a + G5: a reset moves the identity to a new generation and session. The
/// saved scope is refused typed with nothing submitted; recovery with it
/// still reads the original session (completed for the delivered key, absent
/// for the refused one), while the current session has neither key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_scope_is_refused_and_recovery_reads_the_original_session() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;
    let correlation = uuid::Uuid::new_v4().to_string();

    let original_json = boot.status_scope().await;
    let original = DeliveryScope::from_json_value(original_json.clone()).unwrap();
    let delivered = boot
        .scoped_dispatch(
            &original_json,
            "before reset",
            "delivered-key",
            &correlation,
        )
        .await;
    assert!(delivered.get("error").is_none(), "{delivered}");
    next_model_call(&mut calls).await;
    boot.wait_terminal(&original, "delivered-key", &correlation)
        .await;

    boot.identity_rt.reset(&identity()).await.expect("reset");
    let current_json = boot.status_scope().await;
    assert_ne!(
        current_json["member"]["session_id"], original_json["member"]["session_id"],
        "reset moves the identity to a new session"
    );

    let refused = boot
        .scoped_dispatch(&original_json, "after reset", "refused-key", &correlation)
        .await;
    assert_eq!(refused["error"]["code"], -32007, "{refused}");
    assert_eq!(
        refused["error"]["data"]["kind"], "stale_delivery_scope",
        "{refused}"
    );
    assert_eq!(
        refused["error"]["data"]["admission_possible"], false,
        "{refused}"
    );
    assert!(
        ["generation", "runtime_id", "lease_fencing_token"]
            .contains(&refused["error"]["data"]["mismatch"].as_str().unwrap_or("")),
        "{refused}"
    );
    assert_no_model_call(&mut calls, "a stale scope submits nothing");

    let original_delivered = boot
        .recover(&original_json, "delivered-key", &correlation)
        .await;
    assert_eq!(
        original_delivered["recovery"]["state"], "completed",
        "{original_delivered}"
    );
    assert_eq!(original_delivered["recovery"]["output"], ANSWER);
    let original_refused = boot
        .recover(&original_json, "refused-key", &correlation)
        .await;
    assert_eq!(
        original_refused["recovery"]["state"], "absent",
        "{original_refused}"
    );
    let current = boot
        .recover(&current_json, "delivered-key", &correlation)
        .await;
    assert_eq!(current["recovery"]["state"], "absent", "{current}");
    boot.shutdown().await;
}

/// G4 at the native boundary: the member's session moves under the SAME
/// Meta continuity (runtime id, generation and lease unchanged). Meta's
/// own checks pass, and meerkat's admission authority refuses the scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_moved_under_the_same_continuity_is_refused_by_native_admission() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;
    let scope_json = boot.status_scope().await;
    let scope = DeliveryScope::from_json_value(scope_json.clone()).unwrap();

    boot.unified
        .mob_handle()
        .rebind_member_session_for_test(
            scope.member().agent_identity(),
            meerkat_core::types::SessionId::new(),
        )
        .await
        .expect("move the member's session binding");

    let error = boot
        .identity_rt
        .dispatch_at_scope(
            &identity(),
            &scope,
            &DispatchInput::system("moved")
                .with_idempotency("moved-key")
                .with_correlation(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .expect_err("a moved session refuses the saved scope");
    assert!(
        matches!(
            &error,
            ScopedDeliveryError::StaleScope {
                mismatch: meerkat_meta::identity_first::ScopeMismatch::MemberBinding,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(!error.admission_possible());
    assert_no_model_call(&mut calls, "a refused scope submits nothing");
    boot.shutdown().await;
}

/// A delivery during a parked mob Shutdown is a definite refusal. The admitted
/// Shutdown closes the mob's admission before its teardown (held here on the
/// member's session registration), so the scoped submit is refused with
/// `AdmissionClosed`, which the bridge reports as `Rejected`: nothing was
/// admitted, and nothing reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_during_a_parked_shutdown_is_rejected() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;
    let scope = DeliveryScope::from_json_value(boot.status_scope().await).unwrap();
    let machine = boot
        .unified
        .mob_runtime()
        .session_service()
        .expect("the persistent runtime has a session service")
        .acquire_runtime_adapter(None)
        .expect("acquire the runtime adapter")
        .expect("the persistent runtime has a runtime adapter");
    let held = machine
        .hold_session_registration_transaction_for_test(scope.session_id())
        .await;

    let mob = boot.unified.mob_handle();
    let mut closed = mob.admission_closed();
    let shutdown = tokio::spawn({
        let mob = mob.clone();
        async move { mob.shutdown().await }
    });
    tokio::time::timeout(WAIT, closed.wait_for(Option::is_some))
        .await
        .expect("the admitted Shutdown closes admission")
        .expect("the mob's admission state outlives the wait");
    assert_eq!(
        *closed.borrow(),
        Some(meerkat_mob::MobAdmissionCloseReason::Shutdown)
    );

    let error = tokio::time::timeout(
        WAIT,
        boot.identity_rt.dispatch_at_scope(
            &identity(),
            &scope,
            &DispatchInput::system("during the shutdown")
                .with_idempotency("parked-shutdown-key")
                .with_correlation(uuid::Uuid::new_v4().to_string()),
        ),
    )
    .await
    .expect("the delivery is answered while the Shutdown is parked")
    .expect_err("a shutting-down mob admits no delivery");
    assert!(
        matches!(&error, ScopedDeliveryError::Rejected { detail }
            if detail.contains("admission is closed (shutdown)")),
        "{error:?}"
    );
    assert!(!error.admission_possible());
    assert!(
        !shutdown.is_finished(),
        "the Shutdown is still parked on the held registration"
    );
    assert_no_model_call(&mut calls, "a refused delivery submits nothing");

    drop(held);
    tokio::time::timeout(WAIT, shutdown)
        .await
        .expect("the Shutdown finishes once the registration is released")
        .expect("the Shutdown task does not panic")
        .expect("the Shutdown succeeds");
    boot.shutdown().await;
}

/// The scoped path requires a full delivery identity pair, used verbatim,
/// and refuses a scope of another identity, both before anything is
/// submitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_dispatch_refuses_half_pairs_and_foreign_scopes_before_submission() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;
    let scope_json = boot.status_scope().await;

    let half = boot
        .call(
            "meta/dispatch",
            json!({
                "identity": IDENTITY,
                "dispatch_input": {"content": "half", "idempotency_key": "half-key"},
                "expected_scope": scope_json,
            }),
        )
        .await;
    assert_eq!(
        half["error"]["data"]["kind"], "scoped_delivery_rejected",
        "{half}"
    );

    let mut foreign = scope_json.clone();
    foreign["identity"] = json!("personal:bob");
    let foreign_reply = boot
        .scoped_dispatch(
            &foreign,
            "foreign",
            "foreign-key",
            &uuid::Uuid::new_v4().to_string(),
        )
        .await;
    assert_eq!(
        foreign_reply["error"]["data"]["kind"], "scoped_delivery_rejected",
        "{foreign_reply}"
    );

    let mut future = scope_json.clone();
    future["version"] = json!(2);
    let future_reply = boot
        .scoped_dispatch(
            &future,
            "future",
            "future-key",
            &uuid::Uuid::new_v4().to_string(),
        )
        .await;
    assert_eq!(future_reply["error"]["code"], -32602, "{future_reply}");
    assert_eq!(
        future_reply["error"]["data"]["unsupported_version"], 2,
        "{future_reply}"
    );

    let tracked = boot
        .call(
            "meta/dispatch",
            json!({
                "identity": IDENTITY,
                "dispatch_input": {
                    "content": "tracked",
                    "idempotency_key": "tracked-key",
                    "correlation_id": uuid::Uuid::new_v4().to_string(),
                },
                "expected_scope": scope_json,
                "track_turn": true,
            }),
        )
        .await;
    assert_eq!(tracked["error"]["code"], -32602, "{tracked}");
    assert_no_model_call(&mut calls, "every refusal precedes submission");
    boot.shutdown().await;
}

/// Two identities in one mob; Bob has completed one keyed delivery in his
/// own session, which his own scope recovers.
struct BobsCompletedDelivery {
    boot: Boot,
    calls: tokio::sync::mpsc::UnboundedReceiver<()>,
    alice: Value,
    bob: Value,
    key: &'static str,
    correlation: String,
}

async fn bobs_completed_delivery(state_path: &std::path::Path) -> BobsCompletedDelivery {
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start_with(
        state_path,
        &unique_mob_id(),
        &client,
        vec![spec(), spec_for(OTHER)],
    )
    .await;
    let alice = boot.status_scope_of(IDENTITY).await;
    let bob = boot.status_scope_of(OTHER).await;
    assert_ne!(alice["member"]["session_id"], bob["member"]["session_id"]);
    let key = "bob-occurrence";
    let correlation = uuid::Uuid::new_v4().to_string();
    let dispatched = boot
        .scoped_dispatch_as(OTHER, &bob, "for bob", key, &correlation)
        .await;
    assert!(dispatched.get("error").is_none(), "{dispatched}");
    next_model_call(&mut calls).await;
    let bob_scope = DeliveryScope::from_json_value(bob.clone()).unwrap();
    boot.wait_terminal(&bob_scope, key, &correlation).await;
    let genuine = boot.recover_reply(OTHER, &bob, key, &correlation).await;
    assert_eq!(
        genuine["result"]["recovery"]["state"], "completed",
        "{genuine}"
    );
    BobsCompletedDelivery {
        boot,
        calls,
        alice,
        bob,
        key,
        correlation,
    }
}

/// P1, Meta half: a scope selects, it does not authenticate. Alice's
/// envelope around Bob's member scope is refused by Alice's identity-first
/// membership before anything is read, never answered with Bob's delivery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_naming_another_identitys_member_is_refused_before_recovery() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut fixture = bobs_completed_delivery(&temp.path().join("state")).await;
    let mut member_swapped = fixture.alice.clone();
    member_swapped["member"] = fixture.bob["member"].clone();
    let refused = fixture
        .boot
        .recover_reply(IDENTITY, &member_swapped, fixture.key, &fixture.correlation)
        .await;
    assert_eq!(
        refused["error"]["data"]["kind"], "scoped_delivery_rejected",
        "{refused}"
    );
    assert_no_model_call(&mut fixture.calls, "recovery submits nothing");
    fixture.boot.shutdown().await;
}

/// P1, meerkat half: Alice's own member scope with only its session swapped
/// to Bob's names a session Alice's member never held. meerkat's member
/// history does not bind them, so recovery stays unresolved and never
/// reports Bob's delivery under Alice's envelope.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_with_another_members_session_stays_unresolved() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut fixture = bobs_completed_delivery(&temp.path().join("state")).await;
    let mut session_swapped = fixture.alice.clone();
    session_swapped["member"]["session_id"] = fixture.bob["member"]["session_id"].clone();
    let swapped = fixture
        .boot
        .recover_reply(
            IDENTITY,
            &session_swapped,
            fixture.key,
            &fixture.correlation,
        )
        .await;
    assert!(swapped.get("error").is_none(), "{swapped}");
    assert_eq!(
        swapped["result"]["recovery"]["state"], "unresolved",
        "{swapped}"
    );
    assert_eq!(
        swapped["result"]["recovery"]["cause"], "scope_binding_unproven",
        "{swapped}"
    );
    assert_no_model_call(&mut fixture.calls, "recovery submits nothing");
    fixture.boot.shutdown().await;
}

/// P1, meerkat half: a scope another mob captured, for an identity of the
/// same name, is not this mob's. Recovery here stays unresolved and never
/// reads the foreign session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_captured_by_another_mob_stays_unresolved() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, mut calls) = BarrierClient::new(true);
    let home = Boot::start(&temp.path().join("home"), &unique_mob_id(), &client).await;
    let foreign = Boot::start(&temp.path().join("foreign"), &unique_mob_id(), &client).await;
    let scope = foreign.status_scope().await;
    let key = "foreign-occurrence";
    let correlation = uuid::Uuid::new_v4().to_string();
    let dispatched = foreign
        .scoped_dispatch(&scope, "for the foreign mob", key, &correlation)
        .await;
    assert!(dispatched.get("error").is_none(), "{dispatched}");
    next_model_call(&mut calls).await;
    let foreign_scope = DeliveryScope::from_json_value(scope.clone()).unwrap();
    foreign
        .wait_terminal(&foreign_scope, key, &correlation)
        .await;
    let genuine = foreign.recover(&scope, key, &correlation).await;
    assert_eq!(genuine["recovery"]["state"], "completed", "{genuine}");
    let reply = home
        .recover_reply(IDENTITY, &scope, key, &correlation)
        .await;
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(
        reply["result"]["recovery"]["state"], "unresolved",
        "{reply}"
    );
    assert_eq!(
        reply["result"]["recovery"]["cause"], "scope_binding_unproven",
        "{reply}"
    );
    assert_no_model_call(&mut calls, "recovery submits nothing");
    foreign.shutdown().await;
    home.shutdown().await;
}

/// A scope saved by Meta 0.9.3 is refused as a stale scope (`-32007`), never
/// as an invalid one, now that Core writes version-2 member scopes. 0.9.3
/// saved Core's version-1 member scope, which names no mob incarnation, beside
/// Meta's own generation and lease fencing token. Whether those atoms happen
/// to equal the live member's or not, the refusal is the typed stale scope and
/// nothing is submitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_saved_by_meta_0_9_3_is_refused_as_stale() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let current = boot.status_scope().await;
    // The version-1 member scope 0.9.3 saved: it names no mob incarnation.
    let mut legacy = current.clone();
    let member = legacy["member"].as_object_mut().expect("member scope");
    assert!(member.remove("mob_incarnation").is_some(), "{current}");
    member.insert("version".to_string(), json!(1));
    // Meta 0.9.3's own atoms, which are not Core's generation and fence.
    let mut meta_atoms = legacy.clone();
    meta_atoms["generation"] = json!(current["generation"].as_u64().expect("generation") + 7);
    meta_atoms["lease_fencing_token"] = json!(
        current["lease_fencing_token"]
            .as_u64()
            .expect("fence token")
            + 7
    );
    let correlation = uuid::Uuid::new_v4().to_string();
    for (case, key, saved) in [
        ("the live member's atoms", "legacy-live-atoms", &legacy),
        ("Meta 0.9.3 atoms", "legacy-meta-atoms", &meta_atoms),
    ] {
        let refused = boot
            .scoped_dispatch(saved, "saved by 0.9.3", key, &correlation)
            .await;
        assert_eq!(refused["error"]["code"], -32007, "{case}: {refused}");
        assert_eq!(
            refused["error"]["data"]["kind"], "stale_delivery_scope",
            "{case}: {refused}"
        );
        assert_eq!(
            refused["error"]["data"]["admission_possible"], false,
            "{case}: {refused}"
        );
    }
    assert_no_model_call(&mut calls, "a scope saved by 0.9.3 submits nothing");
    boot.shutdown().await;
}

/// Gate 6, part 1 (D3): one key at one scope is one input. A replay with
/// changed bytes never creates a second effect or overwrites the original;
/// rejecting the conflict belongs to part 2's exact-replay witness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_key_at_one_scope_is_one_effect_and_changed_bytes_never_overwrite() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;
    let scope_json = boot.status_scope().await;
    let scope = DeliveryScope::from_json_value(scope_json.clone()).unwrap();
    let correlation = uuid::Uuid::new_v4().to_string();

    let first = boot
        .scoped_dispatch(&scope_json, "original bytes", "one-key", &correlation)
        .await;
    assert!(first.get("error").is_none(), "{first}");
    next_model_call(&mut calls).await;
    boot.wait_terminal(&scope, "one-key", &correlation).await;
    let original = boot
        .identity_rt
        .recover_at_scope(
            &identity(),
            &scope,
            &DispatchInput::system("")
                .with_idempotency("one-key")
                .with_correlation(correlation.clone()),
            WAIT,
        )
        .await
        .unwrap();
    let ScopedRecovery::Completed {
        input_id: original_input,
        output,
    } = &original
    else {
        panic!("the original delivery completed: {original:?}");
    };
    assert_eq!(
        output,
        &TurnOutput::Text {
            text: ANSWER.to_string(),
            truncated: false
        }
    );

    let replay = boot
        .scoped_dispatch(&scope_json, "CHANGED bytes", "one-key", &correlation)
        .await;
    assert!(replay.get("error").is_none(), "{replay}");
    assert_eq!(
        replay["result"]["receipt"], first["result"]["receipt"],
        "{replay}"
    );
    assert_no_model_call(&mut calls, "the replay is not a second effect");
    let after = boot
        .identity_rt
        .recover_at_scope(
            &identity(),
            &scope,
            &DispatchInput::system("")
                .with_idempotency("one-key")
                .with_correlation(correlation.clone()),
            WAIT,
        )
        .await
        .unwrap();
    assert!(
        matches!(&after, ScopedRecovery::Completed { input_id, .. } if input_id == original_input),
        "the original input stands: {after:?}"
    );
    boot.shutdown().await;
}

/// Ruling: the scoped capability refuses a member with no native session
/// binding (remotely hosted or peer-only) with a typed Unsupported, on
/// capture, dispatch and recovery alike, and never falls back to the
/// unscoped submit. Nothing reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_externally_bound_identity_is_unsupported_and_never_falls_back() {
    let temp = tempfile::TempDir::new().unwrap();
    let state_path = temp.path().join("state");
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&state_path, &unique_mob_id(), &client).await;

    // An active, leased, externally bound identity. Its "scope" is forged
    // from the local member's real one: if any path fell back to a submit,
    // it would reach a real member and the model.
    let peer = AgentIdentity::parse("remote:peer").unwrap();
    let mut peer_spec = spec();
    peer_spec.identity = peer.clone();
    peer_spec.backend = Some(meerkat_mob::MobBackendKind::External);
    let runtime_id = meerkat_meta::identity_first::AgentRuntimeId::parse("rt-remote-peer").unwrap();
    boot.identity_rt.register(peer_spec).await;

    let unsupported = |error: &ScopedDeliveryError| {
        matches!(error, ScopedDeliveryError::Unsupported { detail }
            if detail.contains("no native session binding"))
    };

    let captured = boot
        .identity_rt
        .capture_delivery_scope(&peer)
        .await
        .expect_err("an externally bound identity has no scope");
    assert!(unsupported(&captured), "{captured:?}");

    let mut forged = boot.status_scope().await;
    forged["identity"] = json!(peer.as_str());
    forged["agent_runtime_id"] = json!(runtime_id.as_str());
    forged["generation"] = json!(0);
    forged["lease_fencing_token"] = json!(1);
    let forged = DeliveryScope::from_json_value(forged).unwrap();
    let lookup = DispatchInput::system("must never be delivered")
        .with_idempotency("peer-key")
        .with_correlation(uuid::Uuid::new_v4().to_string());

    let dispatched = boot
        .identity_rt
        .dispatch_at_scope(&peer, &forged, &lookup)
        .await
        .expect_err("scoped dispatch refuses an externally bound identity");
    assert!(unsupported(&dispatched), "{dispatched:?}");
    assert!(!dispatched.admission_possible());

    let recovered = boot
        .identity_rt
        .recover_at_scope(&peer, &forged, &lookup, WAIT)
        .await
        .expect_err("scoped recovery refuses an externally bound identity");
    assert!(unsupported(&recovered), "{recovered:?}");

    assert_no_model_call(&mut calls, "no scoped path falls back to a submit");
    boot.shutdown().await;
}

// ---------------------------------------------------------------------------
// Retirement at a captured scope
// ---------------------------------------------------------------------------

/// Capture `IDENTITY`'s scope natively, with its member present in meerkat's
/// roster.
async fn captured_scope(boot: &Boot) -> DeliveryScope {
    let scope = boot
        .identity_rt
        .capture_delivery_scope(&identity())
        .await
        .expect("capture the identity's scope");
    assert!(
        member_present(boot, &scope).await,
        "the captured member is in meerkat's roster"
    );
    scope
}

async fn member_present(boot: &Boot, scope: &DeliveryScope) -> bool {
    boot.unified
        .mob_handle()
        .get_member(scope.member().agent_identity())
        .await
        .expect("read meerkat's roster")
        .is_some()
}

async fn status(boot: &Boot) -> meerkat_meta::identity_first::IdentityStatus {
    boot.identity_rt
        .status(&identity())
        .await
        .expect("the identity is registered")
}

/// Nothing was retired: the identity is active and its member is still in
/// meerkat's roster.
async fn assert_not_retired(boot: &Boot, scope: &DeliveryScope) {
    let status = status(boot).await;
    assert_eq!(status.state, IdentityLifecycleState::Active, "{status:?}");
    assert!(
        member_present(boot, scope).await,
        "the member is still in meerkat's roster"
    );
}

/// An unchanged scope retires the identity, and the retirement has settled
/// when the call returns: the member has left meerkat's roster and the
/// identity rests dormant on its session in Core's history. Retiring again at
/// the same scope converges to `Ok`: no incarnation is current and none is
/// retiring, so a retry after a lost reply is safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unchanged_scope_retires_the_identity_and_settles() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    boot.identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect("an unchanged scope retires the identity");

    let status = status(&boot).await;
    assert_eq!(status.state, IdentityLifecycleState::Dormant, "{status:?}");
    assert_eq!(
        status.session_id.as_ref(),
        Some(scope.session_id()),
        "the continuity keeps the retired session"
    );
    assert!(
        !member_present(&boot, &scope).await,
        "the member has left meerkat's roster"
    );
    let again = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect("a retry at the retired scope converges");
    assert_eq!(again.get(), scope.member().fence_token().get());
    assert!(!member_present(&boot, &scope).await);
    assert_no_model_call(&mut calls, "retirement submits nothing");
    boot.shutdown().await;
}

/// A scope of an identity retired since the capture (by a plain retire, not
/// at the scope) still reaches Core: with no live member there is no live
/// scope for Meta to read, and Core answers `Ok` because no incarnation of
/// the identity is current. The first call converges like a retry, and
/// nothing wakes the identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_of_an_already_retired_identity_converges_to_ok() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, mut calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;
    boot.identity_rt
        .retire_tracked(&identity())
        .await
        .expect("a plain retire");
    assert!(
        !member_present(&boot, &scope).await,
        "the member has left meerkat's roster"
    );

    let fence = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect("a scope of a retired identity converges");
    assert_eq!(fence.get(), scope.member().fence_token().get());
    let status = status(&boot).await;
    assert_eq!(status.state, IdentityLifecycleState::Dormant, "{status:?}");
    assert_eq!(
        status.session_id.as_ref(),
        Some(scope.session_id()),
        "the identity rests on its retired session"
    );
    assert!(
        !member_present(&boot, &scope).await,
        "nothing woke the identity"
    );
    assert_no_model_call(&mut calls, "retirement submits nothing");
    boot.shutdown().await;
}

/// A rebind between capture and retirement moves the identity to another
/// session under the same runtime id and generation, which a retirement by
/// generated alias does not see. The scoped retirement refuses it, with
/// nothing retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebind_between_capture_and_retirement_refuses_the_scoped_retirement() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    // A lower-level rebind moves Core's member to another session under the
    // same runtime id and generation.
    let rebound = meerkat_core::types::SessionId::new();
    boot.unified
        .mob_handle()
        .rebind_member_session_for_test(scope.member().agent_identity(), rebound.clone())
        .await
        .expect("move the member's session binding");
    let rebound_status = status(&boot).await;
    assert_eq!(
        rebound_status.agent_runtime_id.as_ref(),
        Some(scope.agent_runtime_id()),
        "a rebind keeps the runtime id the generated alias names"
    );
    assert_eq!(
        rebound_status.generation.map(meerkat_mob::Generation::get),
        Some(scope.generation().get())
    );

    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("a rebound identity is not retired at the old scope");
    assert!(
        matches!(
            &error,
            ScopedRetireError::StaleScope {
                mismatch: ScopeMismatch::MemberBinding,
                ..
            }
        ),
        "{error:?}"
    );
    assert_not_retired(&boot, &scope).await;
    boot.shutdown().await;
}

/// A reset replaces the identity's member with a new generation and
/// session. The old scope is refused, and the replacement keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replacement_refuses_the_scoped_retirement() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    boot.identity_rt.reset(&identity()).await.expect("reset");
    let replacement = boot
        .identity_rt
        .capture_delivery_scope(&identity())
        .await
        .expect("capture the replacement's scope");
    assert_ne!(replacement.session_id(), scope.session_id());

    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("a replacement is not retired at the old scope");
    assert!(
        matches!(
            &error,
            ScopedRetireError::StaleScope {
                mismatch: ScopeMismatch::RuntimeId | ScopeMismatch::Generation,
                ..
            }
        ),
        "{error:?}"
    );
    assert_not_retired(&boot, &replacement).await;
    assert_eq!(
        status(&boot).await.session_id.as_ref(),
        Some(replacement.session_id())
    );
    boot.shutdown().await;
}

/// A retire and a wake between capture and the scoped retirement resume the
/// same session at Core's next generation: another incarnation is current.
/// The old scope is refused on its runtime id, and the woken member keeps
/// running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_and_wake_refuses_the_old_scope() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    boot.identity_rt
        .retire_tracked(&identity())
        .await
        .expect("retire");
    boot.identity_rt
        .materialize_tracked(&identity())
        .await
        .expect("wake onto the same session");
    let woken = status(&boot).await;
    assert_eq!(woken.session_id.as_ref(), Some(scope.session_id()));
    assert_ne!(
        woken.generation.map(meerkat_mob::Generation::get),
        Some(scope.generation().get()),
        "the wake is Core's next incarnation"
    );

    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("the woken incarnation is not retired at the old scope");
    assert!(
        matches!(
            &error,
            ScopedRetireError::StaleScope {
                mismatch: ScopeMismatch::RuntimeId,
                ..
            }
        ),
        "{error:?}"
    );
    assert_not_retired(&boot, &scope).await;
    boot.shutdown().await;
}

/// meerkat's member moves to another session under the same incarnation (a
/// lower-level rebind). meerkat's scoped retire checks the session in the
/// step that would retire and refuses the retirement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_session_move_refuses_the_scoped_retirement() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    boot.unified
        .mob_handle()
        .rebind_member_session_for_test(
            scope.member().agent_identity(),
            meerkat_core::types::SessionId::new(),
        )
        .await
        .expect("move the member's session binding");

    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("a moved native binding is not retired at the old scope");
    assert!(
        matches!(
            &error,
            ScopedRetireError::StaleScope {
                mismatch: ScopeMismatch::MemberBinding,
                ..
            }
        ),
        "{error:?}"
    );
    assert_not_retired(&boot, &scope).await;
    boot.shutdown().await;
}

/// A scope selects, it does not authenticate: another identity's scope, or
/// an envelope around another identity's member, is refused before anything
/// is read or retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_scope_is_refused_before_anything_is_retired() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start_with(
        &temp.path().join("state"),
        &unique_mob_id(),
        &client,
        vec![spec(), spec_for(OTHER)],
    )
    .await;
    let alice = captured_scope(&boot).await;
    let bob_json = boot.status_scope_of(OTHER).await;
    let bob = DeliveryScope::from_json_value(bob_json.clone()).unwrap();

    let foreign = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &bob)
        .await
        .expect_err("another identity's scope is refused");
    assert!(
        matches!(foreign, ScopedRetireError::Rejected { .. }),
        "{foreign:?}"
    );
    let mut swapped = alice.to_json_value();
    swapped["member"] = bob_json["member"].clone();
    let swapped = DeliveryScope::from_json_value(swapped).unwrap();
    let swapped = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &swapped)
        .await
        .expect_err("an envelope around another member is refused");
    assert!(
        matches!(swapped, ScopedRetireError::Rejected { .. }),
        "{swapped:?}"
    );
    assert_not_retired(&boot, &alice).await;
    assert!(member_present(&boot, &bob).await);
    boot.shutdown().await;
}

/// An identity Core holds an identity intent row for is the host's: the
/// scoped retirement refuses it typed (`Refused` with
/// `IdentityHostAdopted`), and its member keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_adopted_identity_is_refused_by_the_scoped_retirement() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    let handle = boot.unified.mob_handle();
    let core_identity = scope.member().agent_identity().clone();
    let entry = handle
        .get_member(&core_identity)
        .await
        .expect("Core's roster read")
        .expect("a live member to adopt");
    let request = meerkat_mob::AdoptMemberIdentityDeclaration {
        mob_id: handle.definition().id.clone(),
        agent_identity: core_identity,
        request_id: meerkat_mob::IdentityAdoptionId::new("scoped-retire-adoption")
            .expect("valid adoption id"),
        precondition: meerkat_mob::IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: meerkat_mob::IdentityDeclarationScopeId::new("scoped-retire-host")
            .expect("valid declaration scope"),
        declaration_revision: 1,
        session: meerkat_mob::DesiredSessionTarget {
            session_id: scope.session_id().clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(scope.session_id()),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: meerkat_mob::DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: meerkat_mob::IdentityProfileMemberDeclaration {
            profile_name: entry.role.clone(),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: Some(entry.labels.clone()),
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: None,
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: meerkat_mob::DesiredExecution::ControllingSession,
        },
        wiring_custody: Default::default(),
        owned_wiring: Default::default(),
        convergence: meerkat_mob::IdentityConvergenceMode::Drain { max_wait_ms: 1_000 },
    };
    let adopted = handle
        .adopt_member_identity_declaration(request)
        .await
        .expect("Core adopts the live member");
    assert!(
        matches!(
            adopted.adoption,
            meerkat_mob::IdentityAdoptionOutcome::Adopted { .. }
        ),
        "{:?}",
        adopted.adoption
    );

    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("a host-adopted identity is not retired at its scope");
    assert!(
        matches!(
            &error,
            ScopedRetireError::Refused(
                meerkat_meta::identity_first::IdentityRuntimeError::IdentityHostAdopted { .. }
            )
        ),
        "{error:?}"
    );
    assert!(
        member_present(&boot, &scope).await,
        "the adopted member keeps running"
    );
    boot.shutdown().await;
}

/// A runtime that no longer admits identity operations refuses the scoped
/// retirement as `Rejected`, before any of it runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runtime_that_admits_no_operation_rejects_the_scoped_retirement() {
    let temp = tempfile::TempDir::new().unwrap();
    let (client, _calls) = BarrierClient::new(true);
    let boot = Boot::start(&temp.path().join("state"), &unique_mob_id(), &client).await;
    let scope = captured_scope(&boot).await;

    boot.unified.shutdown().await;
    let error = boot
        .identity_rt
        .retire_at_scope_tracked(&identity(), &scope)
        .await
        .expect_err("a runtime that is shutting down retires nothing");
    assert!(
        matches!(error, ScopedRetireError::Rejected { .. }),
        "{error:?}"
    );
}
