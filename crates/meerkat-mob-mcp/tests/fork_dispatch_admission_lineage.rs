//! A host's dispatch admission on the children its gated members fork, end
//! to end over the persistent composition.
//!
//! The host gates every member through its spawn customizer
//! (`MobMcpState::with_child_spawn_member_customizer`). A fork of a gated
//! member by the handle's API, and a local temporary-council branch of one,
//! run their tool calls under the admission the customizer derives for
//! them; a fork child restored after a cold restart still does, because its
//! spawn row records that its lineage runs under an admission. Only the LLM
//! is scripted.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, LiveBridgeEffectKind, Message, ToolDispatchAdmission, ToolDispatchContext,
    ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobError, MobId, MobRuntimeMode, ProfileName,
    SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilParticipantSpec,
    TemporaryCouncilRequest,
};
use serde_json::json;
use support::{
    CouncilFixture, ScriptedTurn, council_definition, identity, last_user_text, role_in_request,
};

/// The one host tool.
const HOST_TOOL: &str = "host_echo";
/// A member turn whose prompt holds this marker and a call id calls the host
/// tool once with that id.
const TASK: &str = "CALL-HOST-TOOL-7Q";

// ===========================================================================
// Host tools, admission and customizer
// ===========================================================================

/// The host's tools: records the id of every call that reaches the host.
struct HostTools(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            HOST_TOOL,
            "Ordinary host tool.",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.lock().unwrap().push(call.id.to_string());
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }
}

/// The host's approval gate: admits every call and records its id.
struct RecordingAdmission(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl ToolDispatchAdmission for RecordingAdmission {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        _context: Option<&ToolDispatchContext>,
        _effect_kind: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        self.0.lock().unwrap().push(call.id.to_string());
        Ok(())
    }
}

/// The host's customizer: installs the gate on every spawn and rebuild it
/// is asked about, and records each request as `(source, identity)`.
struct GateEveryMember {
    admission: Arc<dyn ToolDispatchAdmission>,
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

impl SpawnMemberCustomizer for GateEveryMember {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.asked.lock().unwrap().push((
            ctx.spawn_source.as_str().to_string(),
            spec.identity.to_string(),
        ));
        spec.tool_dispatch_admission = Some(Arc::clone(&self.admission));
        Ok(())
    }
}

/// What one process of the host records.
#[derive(Clone, Default)]
struct Host {
    /// Call ids that reached the host tools.
    ran: Arc<Mutex<Vec<String>>>,
    /// Call ids the host's gate was asked about.
    admitted: Arc<Mutex<Vec<String>>>,
    /// `(spawn source, identity)` of every request the customizer was asked.
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

impl Host {
    /// The host's composition of the mob state: its tools for every mob and
    /// its gating customizer.
    fn compose(&self) -> impl FnOnce(MobMcpState, &std::path::Path) -> MobMcpState {
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(HostTools(Arc::clone(&self.ran)));
        let customizer = Arc::new(GateEveryMember {
            admission: Arc::new(RecordingAdmission(Arc::clone(&self.admitted))),
            asked: Arc::clone(&self.asked),
        });
        move |state, _root| {
            let provider: meerkat_mob::ExternalToolsProvider =
                Arc::new(move || Some(Arc::clone(&tools)));
            state
                .with_external_tools_provider(Some(provider))
                .with_child_spawn_member_customizer(customizer)
        }
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }

    fn admitted(&self) -> Vec<String> {
        self.admitted.lock().unwrap().clone()
    }

    fn was_asked(&self, source: &str, member: &str) -> bool {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .any(|(asked_source, asked)| asked_source == source && asked == member)
    }

    /// The call reached the host, and only through the host's gate.
    fn assert_gated(&self, call_id: &str) {
        assert!(
            self.ran().iter().any(|ran| ran == call_id),
            "the call {call_id} reached the host tool: {:?}",
            self.ran()
        );
        assert!(
            self.admitted().iter().any(|admitted| admitted == call_id),
            "the call {call_id} passed the host's gate: admitted {:?}, asked {:?}",
            self.admitted(),
            self.asked.lock().unwrap()
        );
    }
}

// ===========================================================================
// Script
// ===========================================================================

/// Tool results the model has seen since the last user message.
fn results_this_turn(request: &LlmRequest) -> usize {
    request
        .messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, Message::User(_)))
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results.len()),
            _ => None,
        })
        .sum()
}

/// A member turn whose prompt names `TASK <id>` calls the host tool once
/// with call id `<id>`; a council participant's discussion turn calls it
/// once with call id `council-<role>`; every turn then answers.
fn script(request: &LlmRequest) -> ScriptedTurn {
    let last = last_user_text(request);
    let call_id = match last.find(TASK) {
        Some(at) => Some(last[at + TASK.len()..].trim().to_string()),
        None => role_in_request(request).map(|role| format!("council-{role}")),
    };
    match call_id {
        Some(id) if results_this_turn(request) == 0 => ScriptedTurn::ToolCall {
            id,
            name: HOST_TOOL.to_string(),
            args: json!({}),
        },
        _ => ScriptedTurn::Text("done".to_string()),
    }
}

// ===========================================================================
// Steps
// ===========================================================================

fn child_spec(member: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("participant"), identity(member));
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    spec
}

/// Fork `source` of the fixture's source mob as `child`.
async fn fork(fixture: &CouncilFixture, source: &str, child: &str) {
    fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle")
        .fork_member(&identity(source), child_spec(child), None)
        .await
        .expect("fork the child");
}

/// Run one turn of `member` of `mob_id` (restoring the mob first when this
/// process has not met it yet) that calls the host tool with `call_id`.
async fn run_turn(state: &MobMcpState, mob_id: &MobId, member: &str, call_id: &str) {
    let handle = state.handle_for(mob_id).await.expect("mob handle");
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(member),
            WorkSpec::new(
                ContentInput::Text(format!("{TASK} {call_id}")),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("turn succeeds");
}

// ===========================================================================
// Rows
// ===========================================================================

/// A fork of a gated member by `MobHandle::fork_member` runs its tool calls
/// under the admission the host's customizer derives for it at its seat.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_fork_runs_its_tool_calls_under_the_host_admission() {
    let host = Host::default();
    let fixture = CouncilFixture::new_with(script, host.compose());
    fixture.seed_source_mob(&["source"]).await;
    fork(&fixture, "source", "fork-child").await;
    assert!(
        host.was_asked("persisted_fork_resume", "fork-child"),
        "the host is asked for the child at its seat: {:?}",
        host.asked.lock().unwrap()
    );
    run_turn(
        &fixture.state,
        &fixture.source_mob_id(),
        "fork-child",
        "fork-child-call",
    )
    .await;
    host.assert_gated("fork-child-call");
    fixture.teardown().await;
}

/// A local temporary-council branch of gated members runs its discussion
/// turn's tool calls under the admission the host's customizer derives for
/// the branch when it is seated.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_council_branch_runs_its_tool_calls_under_the_host_admission() {
    let host = Host::default();
    let fixture = CouncilFixture::new_with(script, host.compose());
    fixture.seed_source_mob(&["researcher", "skeptic"]).await;
    let participant = |order, role: &str, source: &str| {
        TemporaryCouncilParticipantSpec::new(
            order,
            role,
            fixture.source_mob_id(),
            identity(source),
            identity(role),
            ProfileName::from("participant"),
        )
    };
    let request = TemporaryCouncilRequest::new(
        fixture.council_id("gated-council"),
        council_definition("template-is-replaced"),
        vec![
            participant(0, "analyst", "researcher"),
            participant(1, "critic", "skeptic"),
        ],
        "Should we ship the migration this week?",
        TemporaryCouncilBounds::relative(Duration::from_mins(2), 1, 4096),
        MergeBackPolicy::NoMerge,
    );
    let outcome = fixture
        .state
        .temporary_council()
        .run(request)
        .await
        .expect("the council runs to a terminal outcome");
    assert_eq!(outcome.result.exchanges.len(), 2, "{:?}", outcome.result);
    for role in ["analyst", "critic"] {
        assert!(
            host.was_asked("attached_forked_participant", role),
            "the host is asked for the {role} branch at its seat: {:?}",
            host.asked.lock().unwrap()
        );
        host.assert_gated(&format!("council-{role}"));
    }
    fixture.teardown().await;
}

/// The restart row: a fork child of a gated member, a cold restart of the
/// whole process, and the child's next turn (which restores it) still runs
/// its tool calls under the admission the restarted host derives for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_child_restored_after_a_cold_restart_still_runs_under_the_host_admission() {
    let first = Host::default();
    let mut fixture = CouncilFixture::new_with(script, first.compose());
    fixture.seed_source_mob(&["source"]).await;
    fork(&fixture, "source", "fork-child").await;
    run_turn(
        &fixture.state,
        &fixture.source_mob_id(),
        "fork-child",
        "first-lifetime-call",
    )
    .await;
    first.assert_gated("first-lifetime-call");

    let restarted = Host::default();
    fixture.restart_cold_with(script, restarted.compose()).await;
    run_turn(
        &fixture.state,
        &fixture.source_mob_id(),
        "fork-child",
        "restored-call",
    )
    .await;
    assert!(
        restarted.was_asked("resume", "fork-child"),
        "the restarted host is asked for the child's own identity: {:?}",
        restarted.asked.lock().unwrap()
    );
    restarted.assert_gated("restored-call");
    fixture.teardown().await;
}
