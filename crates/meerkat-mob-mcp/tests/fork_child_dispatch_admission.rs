//! A host's dispatch admission on mob members and on the children they fork.
//!
//! A host installs its own approval gate on every member as
//! `SpawnMemberSpec::tool_dispatch_admission`, through a
//! `SpawnMemberCustomizer` (`MobMcpState::with_child_spawn_member_customizer`).
//! These tests show where that gate reaches today:
//!
//! - the member's own tool calls pass through it;
//! - a `delegate` helper (a fresh helper spawn) and a `fork_helper` child
//!   (`SpawnSource::Fork`) pass through it, because their spawns ask the
//!   customizer;
//! - a `fork_off` child does not: it is seated through
//!   `SpawnSource::PersistedForkResume`, which the customizer is never asked
//!   for, and `ForkBuildInheritance` carries no `tool_dispatch_admission`, so
//!   the child's tool calls run with no admission at all while its parent's
//!   are gated. The parent's recorded tool restrictions do reach the child.
//!
//! Only the LLM is scripted, over the runtime-backed persistent composition.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, LiveBridgeEffectKind, Message, SessionId, ToolDispatchAdmission,
    ToolDispatchContext, ToolDispatchOutcome, ToolError, ToolNameSet,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, ForkContext, HelperOptions, MobError, MobId, MobRuntimeMode,
    ProfileName, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec, WorkOrigin,
    WorkSpec,
};
use meerkat_mob_mcp::{AgentMobToolSurface, DetachedCompletionDelivery, MobMcpState};
use serde_json::{Value, json};
use support::{ScriptedTurn, last_user_text};

/// A host tool the member's recorded tool policy denies.
const DENIED: &str = "host_lookup";
/// A host tool every member may call.
const ALLOWED: &str = "host_echo";

const PARENT: &str = "alice";
const PARENT_PROMPT: &str = "PARENT-PROBE-6R call your tools";
const FORK_OFF_TASK: &str = "FORK-OFF-TASK-2N call your tools";
const FORK_HELPER_TASK: &str = "FORK-HELPER-TASK-8V call your tools";
const DELEGATE_PROMPT: &str = "DELEGATE-PROBE-4J delegate the work";
const DELEGATE_TASK: &str = "DELEGATE-TASK-1C call your tools";

// ===========================================================================
// Host tools, admission and customizer
// ===========================================================================

/// Ordinary host tools from the mob-wide provider; records the call id of
/// every call that actually reaches the host.
struct HostTools(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [DENIED, ALLOWED]
            .into_iter()
            .map(|name| {
                // Provenance lets a parent's visible set witness the tools as
                // a delegate helper's ceiling.
                Arc::new(
                    ToolDef::new(
                        name,
                        "Ordinary host tool.",
                        json!({"type": "object", "properties": {}}),
                    )
                    .with_provenance(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: name.into(),
                        artifact: None,
                    }),
                )
            })
            .collect::<Vec<_>>()
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

/// The host's approval gate: admits every call and records its call id.
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

/// The host customizer: installs the recording admission on every spawn it
/// is asked about, records which spawns those were, and gives the source
/// member a recorded tool restriction (`DENIED` is denied).
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
        if spec.identity.as_str() == PARENT {
            spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                ToolNameSet::from_iter([DENIED.to_string()]),
            ));
        }
        Ok(())
    }
}

// ===========================================================================
// Script
// ===========================================================================

/// Tool results the model has seen since the last user message: how far the
/// current turn has got, whatever history a fork copied before it.
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

/// A turn that calls `DENIED`, then `ALLOWED`, then answers; call ids are
/// `<who>-denied` and `<who>-allowed`.
fn denied_then_allowed(who: &str, step: usize) -> ScriptedTurn {
    match step {
        0 => ScriptedTurn::ToolCall {
            id: format!("{who}-denied"),
            name: DENIED.to_string(),
            args: json!({}),
        },
        1 => ScriptedTurn::ToolCall {
            id: format!("{who}-allowed"),
            name: ALLOWED.to_string(),
            args: json!({}),
        },
        _ => ScriptedTurn::Text(format!("{who} done")),
    }
}

fn script(request: &LlmRequest) -> ScriptedTurn {
    let last = last_user_text(request);
    let step = results_this_turn(request);
    if last.contains(FORK_OFF_TASK) {
        return denied_then_allowed("fork-off-child", step);
    }
    if last.contains(FORK_HELPER_TASK) {
        return denied_then_allowed("fork-helper-child", step);
    }
    if last.contains(DELEGATE_TASK) {
        // A delegate helper is capped by its parent's visible tools, which
        // do not include DENIED, so it only calls ALLOWED.
        return match step {
            0 => ScriptedTurn::ToolCall {
                id: "delegate-helper-allowed".to_string(),
                name: ALLOWED.to_string(),
                args: json!({}),
            },
            _ => ScriptedTurn::Text("delegate helper done".to_string()),
        };
    }
    if last.contains(PARENT_PROMPT) {
        return denied_then_allowed("parent", step);
    }
    if last.contains(DELEGATE_PROMPT) {
        return match step {
            0 => ScriptedTurn::ToolCall {
                id: "parent-delegate".to_string(),
                name: "delegate".to_string(),
                args: json!({
                    "task": DELEGATE_TASK,
                    "member_id": "delegate-helper",
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
            },
            _ => ScriptedTurn::Text("delegated".to_string()),
        };
    }
    ScriptedTurn::Text("ok".to_string())
}

// ===========================================================================
// Rig
// ===========================================================================

struct Rig {
    state: Arc<MobMcpState>,
    mob_id: MobId,
    _temp: tempfile::TempDir,
    /// Call ids that reached the host tools.
    ran: Arc<Mutex<Vec<String>>>,
    /// Call ids the host admission was consulted for.
    admitted: Arc<Mutex<Vec<String>>>,
    /// `(spawn source, identity)` of every spawn the customizer was asked.
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

impl Rig {
    /// A host mob whose one member, `alice`, mounts the agent mob tools and
    /// runs under the host customizer.
    async fn new() -> Self {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let admitted = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(HostTools(Arc::clone(&ran)));
        let customizer = Arc::new(GateEveryMember {
            admission: Arc::new(RecordingAdmission(Arc::clone(&admitted))),
            asked: Arc::clone(&asked),
        });
        let temp = tempfile::tempdir().expect("temp dir");
        // The runtime-backed composition whose members mount the real agent
        // mob tools, so the member's own turn can call `delegate`.
        let state = support::agent_mob_tools_state(
            temp.path(),
            Arc::new(support::ScriptedCouncilClient::new(script)),
            |state| {
                let provider: meerkat_mob::ExternalToolsProvider =
                    Arc::new(move || Some(Arc::clone(&tools)));
                state
                    .with_external_tools_provider(Some(provider))
                    .with_child_spawn_member_customizer(customizer)
            },
        );
        // One-shot delivery: `fork_off` blocks until its child's turn ends.
        state.set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
        let mob_id = MobId::from(format!("gated-{}", uuid::Uuid::new_v4().simple()));
        let mut profile = support::participant_profile("gated member");
        profile.tools.mob = true;
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            ProfileName::from("participant"),
            meerkat_mob::ProfileBinding::Inline(Box::new(profile)),
        );
        state
            .mob_create_definition(definition)
            .await
            .expect("create the host mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("participant"),
                AgentIdentity::from(PARENT),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("spawn the source member");
        let rig = Self {
            state,
            mob_id,
            _temp: temp,
            ran,
            admitted,
            asked,
        };
        assert!(
            rig.asked()
                .contains(&("consumer".to_string(), PARENT.to_string())),
            "the customizer was asked for the source member: {:?}",
            rig.asked()
        );
        rig
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }

    fn admitted(&self) -> Vec<String> {
        self.admitted.lock().unwrap().clone()
    }

    fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }

    fn was_asked_for(&self, identity: &str) -> bool {
        self.asked().iter().any(|(_, asked)| asked == identity)
    }

    /// Run one turn of the source member with `prompt`.
    async fn parent_turn(&self, prompt: &str) {
        let handle = self
            .state
            .handle_for(&self.mob_id)
            .await
            .expect("source mob handle");
        let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
        let work = handle
            .start_work_for_identity_bounded(
                AgentIdentity::from(PARENT),
                WorkSpec::new(ContentInput::Text(prompt.to_string()), WorkOrigin::Internal),
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

    /// Run the parent's probe turn and prove the gate and the restriction
    /// are on the parent: its ALLOWED call was admitted and ran, its DENIED
    /// call never reached the host.
    async fn prove_parent_is_gated(&self) {
        self.parent_turn(PARENT_PROMPT).await;
        assert!(
            self.admitted().contains(&"parent-allowed".to_string()),
            "the parent's call passed through the host admission: {:?}",
            self.admitted()
        );
        assert_eq!(
            self.ran(),
            ["parent-allowed"],
            "the parent's recorded restriction holds"
        );
    }

    async fn parent_session(&self) -> SessionId {
        self.state
            .handle_for(&self.mob_id)
            .await
            .expect("source mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from(PARENT))
            .await
            .expect("the parent has a bridge session")
    }

    /// The parent's `fork_off` of `fork-off-child`, blocking until the
    /// child's turn ends; returns the tool result.
    async fn fork_off(&self) -> Value {
        let mob_id = self.mob_id.clone();
        let incarnation = self
            .state
            .handle_for(&mob_id)
            .await
            .expect("source mob handle")
            .live_incarnation();
        let session = self.parent_session().await;
        let surface = forker_surface(&self.state, session, mob_id.as_str(), incarnation);
        let raw = serde_json::value::RawValue::from_string(
            json!({"member_id": "fork-off-child", "task": FORK_OFF_TASK}).to_string(),
        )
        .unwrap();
        let outcome = tokio::time::timeout(
            Duration::from_mins(1),
            surface.dispatch(ToolCallView {
                id: "surface-fork-off",
                name: "fork_off",
                args: &raw,
            }),
        )
        .await
        .expect("fork_off returns within the failure bound")
        .expect("fork_off succeeds");
        serde_json::from_str(&outcome.result.text_content()).expect("json tool result")
    }

    async fn teardown(self) {
        let _ = self.state.mob_destroy(&self.mob_id).await;
    }
}

/// The agent-facing mob tools of `session` with spawn authority over its own
/// role (what `fork_off` needs), bound the way the agent loop binds them.
fn forker_surface(
    state: &Arc<MobMcpState>,
    session: SessionId,
    mob_id: &str,
    incarnation: meerkat_core::service::LiveMobIncarnation,
) -> Arc<dyn AgentToolDispatcher> {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    let authority =
        meerkat_runtime::mob_operator_authority::set_create_authority(&authority, false)
            .expect("no create scope");
    let authority = meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &authority,
        mob_id,
        incarnation,
        "participant",
    )
    .expect("spawnable participant profile");
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

/// The `fork_off` child ran its turn: the tool result is completed, and its
/// ALLOWED call reached the host while its DENIED call did not (the parent's
/// recorded restriction reached the child).
fn assert_fork_off_child_ran_restricted(rig: &Rig, forked: &Value) {
    assert_eq!(forked["bounded_result"]["status"], "completed", "{forked}");
    assert!(
        rig.ran().contains(&"fork-off-child-allowed".to_string()),
        "the fork_off child called the host tool: {:?}",
        rig.ran()
    );
    assert!(
        !rig.ran().contains(&"fork-off-child-denied".to_string()),
        "the parent's recorded restriction reached the fork_off child: {:?}",
        rig.ran()
    );
}

// ===========================================================================
// Tests
// ===========================================================================

/// Setup proof: the member's own tool calls pass through the admission the
/// host installed through the customizer, under its recorded restriction.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_tool_call_passes_the_host_admission() {
    let rig = Rig::new().await;
    rig.prove_parent_is_gated().await;
    rig.teardown().await;
}

/// Intended behaviour: a `fork_off` child runs under its source's host
/// admission (inherited, or installed by a customizer call for the fork
/// seat), as well as under its source's recorded tool restrictions.
///
/// Delete `today_a_fork_off_child_runs_its_tools_with_no_host_admission`
/// when this passes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a fork_off child does not inherit its source's tool_dispatch_admission, and the fork seat never asks the customizer; see the PR"]
async fn a_fork_off_child_runs_its_tools_under_the_host_admission() {
    let rig = Rig::new().await;
    rig.prove_parent_is_gated().await;
    let forked = rig.fork_off().await;
    assert_fork_off_child_ran_restricted(&rig, &forked);
    assert!(
        rig.admitted()
            .contains(&"fork-off-child-allowed".to_string()),
        "the fork_off child's tool call passed through the host admission: admitted {:?}, \
         customizer asked {:?}",
        rig.admitted(),
        rig.asked()
    );
    rig.teardown().await;
}

/// Today: a `fork_off` child's tool call runs with the host admission never
/// consulted, and the customizer is never asked for the child (its seat is
/// `SpawnSource::PersistedForkResume`), although the parent's own calls are
/// gated and the parent's recorded restriction does reach the child.
///
/// Pins current behaviour; delete it with the fix, when
/// `a_fork_off_child_runs_its_tools_under_the_host_admission` passes.
#[tokio::test(flavor = "multi_thread")]
async fn today_a_fork_off_child_runs_its_tools_with_no_host_admission() {
    let rig = Rig::new().await;
    rig.prove_parent_is_gated().await;
    let admitted_before = rig.admitted();
    let forked = rig.fork_off().await;
    assert_fork_off_child_ran_restricted(&rig, &forked);
    assert!(
        !rig.was_asked_for("fork-off-child"),
        "the customizer is not asked for the fork_off child: {:?}",
        rig.asked()
    );
    let mut admitted_after = rig.admitted();
    // The fork_off call itself came from the test, not a model turn, so the
    // parent's admission saw nothing new either.
    admitted_after.retain(|id| !admitted_before.contains(id));
    assert!(
        admitted_after.is_empty(),
        "no call of the fork_off child was admitted: {admitted_after:?}"
    );
    rig.teardown().await;
}

/// A prompt-context fork (`fork_helper`) is spawned as `SpawnSource::Fork`,
/// which asks the customizer, so its child's tool calls pass through the
/// host admission. The parent's restriction does not travel on this path;
/// the child is built from the role, which is why only ALLOWED is checked.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_helper_child_runs_its_tools_under_the_host_admission() {
    let rig = Rig::new().await;
    rig.prove_parent_is_gated().await;
    let mut options = HelperOptions::default();
    options.role_name = Some(ProfileName::from("participant"));
    let outcome = tokio::time::timeout(
        Duration::from_mins(1),
        rig.state.mob_fork_helper(
            &rig.mob_id,
            &AgentIdentity::from(PARENT),
            AgentIdentity::from("fork-helper-child"),
            FORK_HELPER_TASK.to_string(),
            ForkContext::FullHistory,
            options,
            "fork_helper_result",
            4096,
        ),
    )
    .await
    .expect("fork_helper returns within the failure bound")
    .expect("fork_helper succeeds");
    assert!(
        outcome.retirement_error.is_none(),
        "{:?}",
        outcome.retirement_error
    );
    assert!(
        rig.asked()
            .contains(&("fork".to_string(), "fork-helper-child".to_string())),
        "the customizer was asked for the fork_helper child: {:?}",
        rig.asked()
    );
    assert!(
        rig.ran().contains(&"fork-helper-child-allowed".to_string()),
        "the fork_helper child called the host tool: {:?}",
        rig.ran()
    );
    assert!(
        rig.admitted()
            .contains(&"fork-helper-child-allowed".to_string()),
        "the fork_helper child's call passed through the host admission: {:?}",
        rig.admitted()
    );
    rig.teardown().await;
}

/// Control: a `delegate` helper spawned from the member's own turn is
/// customized like any fresh spawn, so its tool call passes through the host
/// admission. Against `fork_off` this shows the asymmetry is in the fork
/// seat, not in children as such.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegate_helper_runs_its_tools_under_the_host_admission() {
    let rig = Rig::new().await;
    rig.prove_parent_is_gated().await;
    rig.parent_turn(DELEGATE_PROMPT).await;
    assert!(
        rig.admitted().contains(&"parent-delegate".to_string()),
        "the parent's delegate call passed through the host admission: {:?}",
        rig.admitted()
    );
    assert!(
        rig.was_asked_for("delegate-helper"),
        "the customizer was asked for the delegate helper: {:?}",
        rig.asked()
    );
    assert!(
        rig.ran().contains(&"delegate-helper-allowed".to_string()),
        "the delegate helper called the host tool: {:?}",
        rig.ran()
    );
    assert!(
        rig.admitted()
            .contains(&"delegate-helper-allowed".to_string()),
        "the delegate helper's call passed through the host admission: {:?}",
        rig.admitted()
    );
    rig.teardown().await;
}
