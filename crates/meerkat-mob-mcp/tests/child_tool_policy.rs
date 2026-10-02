//! The host's application tool policy governs members of child mobs. A
//! managed host (a consequence-policy registry is installed) must choose a
//! child policy, possibly an explicit Unmanaged; without one, child mob
//! creation is refused, so a constrained member cannot create unconstrained
//! children. Callers never set the binding.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, ApplicationToolPolicyBinding, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    PolicyRevision, ToolConsequenceDenial, ToolConsequenceFailure, ToolConsequenceNarrowingPolicy,
    ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot, ToolConsequenceRequest,
    ToolConsequenceVerdict, ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{MobMcpState, handle_public_tools_call};
use serde_json::json;
use support::{CouncilFixture, ScriptedTurn};

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

async fn create_child(fixture: &CouncilFixture) -> Result<String, meerkat_mob_mcp::McpToolError> {
    let mob_id = format!("child-{}", fixture.scope);
    handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_create",
        &json!({ "definition": {
            "id": mob_id,
            "profiles": { "worker": {
                "model": "claude-sonnet-4-6",
                "runtime_mode": "turn_driven",
                "tools": { "comms": true }
            } }
        } }),
    )
    .await
    .map(|_| mob_id)
}

/// Spawn one child member and run one turn; return the tools that ran.
async fn run_child_turn(fixture: &CouncilFixture, dispatched: &Mutex<Vec<String>>) -> Vec<String> {
    let mob_id = create_child(fixture).await.expect("child mob is created");
    handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_spawn",
        &json!({ "mob_id": mob_id, "profile": "worker", "agent_identity": "child-worker" }),
    )
    .await
    .expect("spawn the child member");
    let handle = fixture
        .state
        .handle_for(&MobId::from(mob_id.as_str()))
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
    dispatched.lock().unwrap().clone()
}

async fn refused(fixture: &CouncilFixture, needle: &str) {
    let error = create_child(fixture)
        .await
        .expect_err("child mob creation is refused");
    assert_eq!(error.code, -32602, "{error:?}");
    assert!(error.message.contains(needle), "{}", error.message);
    assert!(
        fixture
            .state
            .handle_for(&MobId::from(format!("child-{}", fixture.scope).as_str()))
            .await
            .is_err(),
        "no child mob is created"
    );
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
async fn a_managed_host_without_a_child_policy_refuses_child_creation() {
    let (fixture, _dispatched) =
        fixture(|state| state.with_tool_consequence_policy_registry(registry()));
    refused(&fixture, "with_child_application_tool_policy").await;
    // The message names the exact fix: the API, the MobKit init key, and the
    // explicit Unmanaged opt-out.
    let message = create_child(&fixture)
        .await
        .expect_err("still refused")
        .message;
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
    refused(&fixture, "no tool consequence policy registry").await;
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
