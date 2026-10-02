//! The host's application tool policy governs members of child mobs: mobs a
//! member creates with the agent `mob_create` tool. A managed host (a
//! consequence-policy registry is installed) must choose a child policy,
//! possibly an explicit Unmanaged; without one, the agent's `mob_create` is
//! refused with a typed tool error, so a constrained member cannot create
//! unconstrained children. Callers never set the binding, and nothing but
//! child mobs is governed.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, ApplicationToolPolicyBinding, Message, PolicyDigest,
    PolicyEvaluationProvenance, PolicyEvaluationSupervisorConfig, PolicyId,
    PolicyProviderGeneration, PolicyProviderId, PolicyRevision, SessionId, ToolConsequenceDenial,
    ToolConsequenceFailure, ToolConsequenceNarrowingPolicy, ToolConsequencePolicyRegistry,
    ToolConsequencePolicySnapshot, ToolConsequenceRequest, ToolConsequenceVerdict,
    ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, MobRuntimeMode, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{AgentMobToolSurface, DetachedCompletionDelivery, MobMcpState};
use serde_json::{Value, json};
use support::{CouncilFixture, ScriptedTurn, last_user_text, role_in_request, user_text};

const DENIED: &str = "host_lookup";
const ALLOWED: &str = "host_echo";

/// Ordinary external tools from the host's mob-wide provider; records every
/// call that actually reaches it.
struct HostTools(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [DENIED, ALLOWED]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "Ordinary host tool.",
                    json!({"type": "object", "properties": {}}),
                ))
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.lock().unwrap().push(call.name.to_string());
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// A host policy that denies `DENIED` for every member it governs.
struct DenyOne;

impl ToolConsequencePolicySnapshot for DenyOne {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: PolicyRevision(1),
            digest: PolicyDigest::from_canonical_bytes(b"deny-one"),
        }
    }

    fn evaluate(&self, request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        if request.tool_name.as_str() == DENIED {
            ToolConsequenceVerdict::Deny(ToolConsequenceDenial::new(
                "host_policy_denied",
                "the host policy denies this tool",
            ))
        } else {
            ToolConsequenceVerdict::Allow
        }
    }
}

struct HostProvider(PolicyProviderId);

impl ToolConsequenceNarrowingPolicy for HostProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.0
    }

    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(1)
    }

    fn snapshot(
        &self,
        _policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        Ok(Arc::new(DenyOne))
    }
}

fn registry() -> Arc<ToolConsequencePolicyRegistry> {
    Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostProvider(
                PolicyProviderId::new("host").unwrap(),
            ))],
            PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .unwrap(),
    )
}

fn host_policy() -> ApplicationToolPolicyBinding {
    ApplicationToolPolicyBinding::Provider {
        provider_id: PolicyProviderId::new("host").unwrap(),
        policy_id: PolicyId::new("child-tools").unwrap(),
    }
}

/// The agent-facing mob tools of `session`, bound to an operation registry
/// the way the agent loop binds every dispatcher.
fn agent_surface(
    state: &Arc<MobMcpState>,
    session: SessionId,
    authority: meerkat_core::service::MobToolAuthorityContext,
) -> Arc<dyn AgentToolDispatcher> {
    let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
        Arc::clone(state),
        None,
        authority,
        "claude-sonnet-4-5".to_string(),
        session.clone(),
        None,
        None,
        None,
    ));
    surface
        .bind_ops_lifecycle(
            Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
            session,
        )
        .expect("bind ops lifecycle")
        .into_dispatcher()
}

/// Create scope only, as a member that may create mobs holds.
fn creator_authority() -> meerkat_core::service::MobToolAuthorityContext {
    meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority")
}

/// Spawn of its own role in `mob_id`, no create scope.
fn forker_authority(mob_id: &str) -> meerkat_core::service::MobToolAuthorityContext {
    let authority =
        meerkat_runtime::mob_operator_authority::set_create_authority(&creator_authority(), false)
            .expect("no create scope");
    meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &authority,
        mob_id,
        "participant",
    )
    .expect("spawnable participant profile")
}

/// Create scope plus manage scope over `mob_id`, as a council convener holds.
fn convener_authority(mob_id: &str) -> meerkat_core::service::MobToolAuthorityContext {
    meerkat_runtime::mob_operator_authority::grant_manage_mob(&creator_authority(), mob_id)
        .expect("manage scope")
}

async fn dispatch(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    surface
        .dispatch(ToolCallView {
            id: "surface-call",
            name,
            args: &raw,
        })
        .await
}

fn child_definition(mob_id: &str) -> Value {
    json!({
        "id": mob_id,
        "profiles": { "worker": {
            "model": "claude-sonnet-4-6",
            "tools": { "comms": true }
        } }
    })
}

/// The child member calls `DENIED`, then `ALLOWED`, then answers.
fn fixture(
    configure: impl FnOnce(MobMcpState) -> MobMcpState,
) -> (CouncilFixture, Arc<Mutex<Vec<String>>>) {
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(HostTools(Arc::clone(&dispatched)));
    let step = AtomicUsize::new(0);
    let fixture = CouncilFixture::new_with(
        move |_request| match step.fetch_add(1, Ordering::SeqCst) {
            0 => ScriptedTurn::ToolCall {
                id: "call-denied".to_string(),
                name: DENIED.to_string(),
                args: json!({}),
            },
            1 => ScriptedTurn::ToolCall {
                id: "call-allowed".to_string(),
                name: ALLOWED.to_string(),
                args: json!({}),
            },
            _ => ScriptedTurn::Text("done".to_string()),
        },
        move |state, _root| {
            let provider: meerkat_mob::ExternalToolsProvider =
                Arc::new(move || Some(Arc::clone(&tools)));
            configure(state.with_external_tools_provider(Some(provider)))
        },
    );
    (fixture, dispatched)
}

/// The agent `mob_create` of a member of a host-created mob.
async fn create_child(fixture: &CouncilFixture) -> Result<String, ToolError> {
    fixture.seed_source_mob(&["creator"]).await;
    let session = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle")
        .resolve_bridge_session_id(&AgentIdentity::from("creator"))
        .await
        .expect("creator has a bridge session");
    let surface = agent_surface(&fixture.state, session, creator_authority());
    let mob_id = format!("child-{}", fixture.scope);
    dispatch(
        &surface,
        "mob_create",
        json!({ "definition": child_definition(&mob_id) }),
    )
    .await
    .map(|_| mob_id)
}

/// Spawn one child member and run one turn; return the tools that ran.
async fn run_child_turn(fixture: &CouncilFixture, dispatched: &Mutex<Vec<String>>) -> Vec<String> {
    let mob_id = create_child(fixture).await.expect("child mob is created");
    let mob_id = MobId::from(mob_id.as_str());
    fixture
        .state
        .mob_spawn(
            &mob_id,
            "worker".into(),
            AgentIdentity::from("child-worker"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("spawn the child member");
    let handle = fixture
        .state
        .handle_for(&mob_id)
        .await
        .expect("child mob handle");
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("child-worker"),
            WorkSpec::new(ContentInput::Text("go".to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("turn succeeds");
    let ran = dispatched.lock().unwrap().clone();
    fixture.teardown().await;
    ran
}

/// The agent's `mob_create` is refused with a typed policy denial carrying
/// `code`, and nothing is created.
async fn refused(fixture: &CouncilFixture, code: &str) -> String {
    let error = create_child(fixture)
        .await
        .expect_err("child mob creation is refused");
    let ToolError::PolicyDenied { denial } = &error else {
        panic!("a typed policy denial, got {error:?}");
    };
    assert_eq!(denial.code, code, "{error:?}");
    assert!(
        fixture
            .state
            .handle_for(&MobId::from(format!("child-{}", fixture.scope).as_str()))
            .await
            .is_err(),
        "no child mob is created"
    );
    let message = denial.message.clone();
    fixture.teardown().await;
    message
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_child_policy_governs_a_child_members_ordinary_external_tool() {
    let (fixture, dispatched) = fixture(|state| {
        state
            .with_tool_consequence_policy_registry(registry())
            .with_child_application_tool_policy(host_policy())
    });
    assert_eq!(run_child_turn(&fixture, &dispatched).await, [ALLOWED]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_managed_host_without_a_child_policy_refuses_the_agents_mob_create() {
    let (fixture, _dispatched) =
        fixture(|state| state.with_tool_consequence_policy_registry(registry()));
    let message = refused(&fixture, "child_tool_policy_required").await;
    // The message says why before it names the exact fix: the API, the
    // MobKit init key, and the explicit Unmanaged opt-out.
    let why = "this host runs a tool-policy registry and no child application tool policy is \
               configured";
    assert!(message.starts_with(why), "{message}");
    for needle in [
        "MobMcpState::with_child_application_tool_policy(binding)",
        "`child_application_tool_policy` init parameter",
        "ApplicationToolPolicyBinding::Unmanaged",
    ] {
        assert!(message.contains(needle), "{needle} missing from: {message}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unmanaged_host_keeps_unconstrained_children() {
    let (fixture, dispatched) = fixture(|state| state);
    assert_eq!(
        run_child_turn(&fixture, &dispatched).await,
        [DENIED, ALLOWED]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_child_policy_without_a_registry_is_refused() {
    let (fixture, _dispatched) =
        fixture(|state| state.with_child_application_tool_policy(host_policy()));
    let message = refused(&fixture, "child_tool_policy_registry_missing").await;
    assert!(
        message.contains("no tool consequence policy registry"),
        "{message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_managed_host_may_explicitly_choose_unmanaged_children() {
    let (fixture, dispatched) = fixture(|state| {
        state
            .with_tool_consequence_policy_registry(registry())
            .with_child_application_tool_policy(ApplicationToolPolicyBinding::Unmanaged)
    });
    assert_eq!(
        run_child_turn(&fixture, &dispatched).await,
        [DENIED, ALLOWED]
    );
}

const CHILD_TASK: &str = "CHILD-TASK-5P reply with the token";
const CHILD_REPLY: &str = "FORKED-RESULT-3K";
const COUNCIL_SUMMARY: &str = "COUNCIL-SUMMARY-2W the council agreed";

fn same_mob_script(request: &LlmRequest) -> ScriptedTurn {
    if last_user_text(request).contains(CHILD_TASK) {
        return ScriptedTurn::Text(CHILD_REPLY.to_string());
    }
    if user_text(request).contains("bounded plain-text summary") {
        return ScriptedTurn::Text(COUNCIL_SUMMARY.to_string());
    }
    if let Some(role) = role_in_request(request) {
        return ScriptedTurn::Text(format!("position from {role}"));
    }
    ScriptedTurn::Text("ok".to_string())
}

/// Condition (a): a managed host without a child policy refuses only child
/// mobs. Host spawns, same-mob `mob_spawn_member`, `fork_off` and councils
/// keep working.
#[tokio::test(flavor = "multi_thread")]
async fn a_managed_host_without_a_child_policy_leaves_same_mob_spawns_forks_and_councils_alone() {
    let fixture = CouncilFixture::new_runtime_backed_with(same_mob_script, |state, _root| {
        state.with_tool_consequence_policy_registry(registry())
    });
    // Host spawns into a host-created mob.
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let mob_id = fixture.source_mob_id();
    let handle = fixture
        .state
        .handle_for(&mob_id)
        .await
        .expect("source mob handle");
    let session_of = |member: &'static str| {
        let handle = handle.clone();
        async move {
            handle
                .resolve_bridge_session_id(&AgentIdentity::from(member))
                .await
                .unwrap_or_else(|| panic!("{member} has a bridge session"))
        }
    };

    let convener = agent_surface(
        &fixture.state,
        session_of("convener").await,
        convener_authority(mob_id.as_str()),
    );
    dispatch(
        &convener,
        "mob_spawn_member",
        json!({
            "mob_id": mob_id.as_str(),
            "profile": "participant",
            "member_id": "same-mob-peer",
            // Privileged argument: needs manage scope.
            "runtime_mode": "turn_driven",
        }),
    )
    .await
    .expect("same-mob mob_spawn_member is untouched");

    let forker = agent_surface(
        &fixture.state,
        session_of("alice").await,
        forker_authority(mob_id.as_str()),
    );
    let forked = dispatch(
        &forker,
        "fork_off",
        json!({"member_id": "same-mob-fork", "task": CHILD_TASK}),
    )
    .await
    .expect("fork_off is untouched");
    let forked: Value = serde_json::from_str(&forked.result.text_content()).unwrap();
    assert_eq!(forked["bounded_result"]["text"], CHILD_REPLY, "{forked}");

    let council = dispatch(
        &convener,
        "council",
        json!({
            "topic": "Should we ship the migration this week?",
            "participants": [
                {"mob_id": mob_id.as_str(), "member_id": "alice", "role": "analyst"},
                {"mob_id": mob_id.as_str(), "member_id": "bob", "role": "critic"},
            ],
            "max_rounds": 1,
            "timeout_seconds": 120,
            "council_id": fixture.council_id("child-policy").as_str(),
        }),
    )
    .await
    .expect("councils are untouched");
    let council: Value = serde_json::from_str(&council.result.text_content()).unwrap();
    assert!(
        council["result"].to_string().contains(COUNCIL_SUMMARY),
        "{council}"
    );
    fixture.teardown().await;
}

/// Condition (b): the refusal reaches the model as a typed tool error result
/// and the turn goes on.
#[tokio::test(flavor = "multi_thread")]
async fn the_refusal_reaches_the_model_as_a_tool_error_and_the_turn_continues() {
    // The member's own mob tools, built once the state exists.
    let state_slot: Arc<OnceLock<Weak<MobMcpState>>> = Arc::new(OnceLock::new());
    let slot = Arc::clone(&state_slot);
    let provider: meerkat_mob::ExternalToolsProvider = Arc::new(move || {
        // Unbound: the member's agent loop binds its dispatchers.
        let state = slot.get()?.upgrade()?;
        let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
            state,
            None,
            creator_authority(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        ));
        Some(surface)
    });
    let seen = Arc::new(Mutex::new(None::<ToolResult>));
    let sink = Arc::clone(&seen);
    let child_mob_id = Arc::new(OnceLock::<String>::new());
    let child_id = Arc::clone(&child_mob_id);
    let fixture = CouncilFixture::new_with(
        move |request| {
            let tool_result = request
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::ToolResults { results, .. } => results.first().cloned(),
                    _ => None,
                });
            match tool_result {
                None => ScriptedTurn::ToolCall {
                    id: "call-create".to_string(),
                    name: "mob_create".to_string(),
                    args: json!({ "definition": child_definition(child_id.get().unwrap()) }),
                },
                Some(result) => {
                    *sink.lock().unwrap() = Some(result);
                    ScriptedTurn::Text("carried on".to_string())
                }
            }
        },
        move |state, _root| {
            state
                .with_tool_consequence_policy_registry(registry())
                .with_external_tools_provider(Some(provider))
        },
    );
    state_slot.set(Arc::downgrade(&fixture.state)).unwrap();
    child_mob_id
        .set(format!("child-{}", fixture.scope))
        .unwrap();
    fixture.seed_source_mob(&["creator"]).await;

    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle");
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("creator"),
            WorkSpec::new(
                ContentInput::Text("create a child mob".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    let result = tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("the refusal does not fail the turn");
    assert_eq!(result.result().result().text(), "carried on");

    let seen = seen
        .lock()
        .unwrap()
        .clone()
        .expect("the model saw a tool result");
    assert!(seen.is_error, "{seen:?}");
    let content = seen.text_content();
    for needle in [
        "policy_denied",
        "child_tool_policy_required",
        "this host runs a tool-policy registry",
    ] {
        assert!(content.contains(needle), "{needle} missing from: {content}");
    }
    assert!(
        fixture
            .state
            .handle_for(&MobId::from(child_mob_id.get().unwrap().as_str()))
            .await
            .is_err(),
        "no child mob is created"
    );
    fixture.teardown().await;
}

/// `delegate` from a member of a host-created mob, with an explicit helper
/// profile.
async fn delegate(fixture: &CouncilFixture) -> (Result<ToolDispatchOutcome, ToolError>, SessionId) {
    fixture.seed_source_mob(&["creator"]).await;
    let session = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle")
        .resolve_bridge_session_id(&AgentIdentity::from("creator"))
        .await
        .expect("creator has a bridge session");
    let surface = agent_surface(&fixture.state, session.clone(), creator_authority());
    let outcome = tokio::time::timeout(
        Duration::from_secs(60),
        dispatch(
            &surface,
            "delegate",
            json!({
                "task": "go",
                "member_id": "helper",
                "result_label": "helper_result",
                "max_text_bytes": 4096,
                "tooling": {
                    "mode": "profile",
                    "source": {
                        "type": "inline",
                        "model": "claude-sonnet-4-6",
                        "tools": { "comms": true }
                    }
                }
            }),
        ),
    )
    .await
    .expect("delegate returns within the failure bound");
    (outcome, session)
}

/// Delegate helpers are child members: a helper runs under the host's child
/// policy, not unmanaged.
#[tokio::test(flavor = "multi_thread")]
async fn delegate_helpers_run_under_the_host_child_policy() {
    let (fixture, dispatched) = fixture(|state| {
        state
            .with_tool_consequence_policy_registry(registry())
            .with_child_application_tool_policy(host_policy())
    });
    let (outcome, _session) = delegate(&fixture).await;
    outcome.expect("delegate succeeds");
    assert_eq!(*dispatched.lock().unwrap(), [ALLOWED]);
    fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_managed_host_without_a_child_policy_refuses_delegate() {
    let (fixture, dispatched) =
        fixture(|state| state.with_tool_consequence_policy_registry(registry()));
    let (outcome, session) = delegate(&fixture).await;
    let error = outcome.expect_err("delegate is refused");
    let ToolError::PolicyDenied { denial } = &error else {
        panic!("a typed policy denial, got {error:?}");
    };
    assert_eq!(denial.code, "child_tool_policy_required", "{error:?}");
    assert!(
        denial.message.starts_with(
            "this host runs a tool-policy registry and no child application tool policy is \
             configured"
        ),
        "{}",
        denial.message
    );
    assert!(
        fixture
            .state
            .find_implicit_mob_for_bridge_session(&session.to_string())
            .await
            .is_none(),
        "no implicit mob is created"
    );
    assert!(dispatched.lock().unwrap().is_empty());
    fixture.teardown().await;
}
