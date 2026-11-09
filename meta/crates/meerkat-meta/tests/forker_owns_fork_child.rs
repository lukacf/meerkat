//! A forker observes and retires its own fork child through the agent mob
//! tools, on the gateway's persistent composition (an identity-first
//! `domain:calendar`, roster id `mk--domain_ccalendar`, persistent mob
//! storage), driven by a scripted model through the real tool dispatch.
//!
//! A downstream app's 0.8.43 candidate recheck saw `mob_check_member` and
//! `mob_retire_member` on calendar's fork child return access_denied. The child
//! had already been idle-retired, and meerkat's owned-member admission used to
//! report a target no longer on the roster as access_denied. Since meerkat
//! #1234 it observes presence first: an absent target is the typed tool error
//! `execution_failed` with `data.kind` `member_retired` or `member_not_found`,
//! and access_denied only means a present member the caller does not own.
//! The first two tests pin that ownership itself works, in one process and
//! across a restart; the third pins the typed answer for a retired child.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::{HandlingMode, StopReason};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::bridge::MobSessionBridge;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::{RestoreOutcome, restore_flow};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, ContinuityStore,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, ManagedPeerEdge, SessionBridge, TopologyContext,
    TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Each test boots its own mob: tests in one binary share a process, and a
/// mob id names its supervisor's process-wide participant
/// (`<mob>/__mob_supervisor__`), so a shared id would collide.
const MOB_ID_PREFIX: &str = "forker-owns-child";
const FORK_TASK: &str = "FORK-TASK-MARKER: summarise the calendar";
const CHILD: &str = "calendar-fork";

fn mob_toml(mob_id: &str) -> String {
    format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.domain]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.domain.tools]
comms = true
mob = true
"#
    )
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

/// Single turn: fork, check, retire. Fork only: fork, then answer.
/// After restart: check, then retire, in a new turn.
const PHASE_SINGLE_TURN: usize = 0;
const PHASE_FORK_ONLY: usize = 1;
const PHASE_CHECK_AND_RETIRE: usize = 2;
/// The user prompt that starts the check-and-retire turn. Other forker turns
/// in that phase (a late fork completion) make no calls.
const CHECK_PROMPT: &str = "check and retire";

/// The forker forks, checks, then retires its child; the child answers.
#[derive(Clone)]
struct ForkerScript {
    /// The test's own mob id (see [`MOB_ID_PREFIX`]).
    mob_id: Arc<str>,
    /// The forker's tool results, in call order (serialized messages).
    forker_tool_results: Arc<Mutex<Vec<String>>>,
    phase: Arc<std::sync::atomic::AtomicUsize>,
}

impl ForkerScript {
    fn for_test(test: &str) -> Self {
        Self {
            mob_id: format!("{MOB_ID_PREFIX}-{}", test.replace('_', "-")).into(),
            forker_tool_results: Arc::default(),
            phase: Arc::default(),
        }
    }
}

impl meerkat_client::LlmClient for ForkerScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let is_child = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .is_some_and(|text| text.contains(FORK_TASK));
        let tool_results: Vec<&Message> = request
            .messages
            .iter()
            .filter(|message| matches!(message, Message::ToolResults { .. }))
            .collect();
        if !is_child && let Some(last) = tool_results.last() {
            let last = serde_json::to_string(last).unwrap();
            let mut recorded = self.forker_tool_results.lock().unwrap();
            if !recorded.contains(&last) {
                recorded.push(last);
            }
        }
        let last_user = request
            .messages
            .iter()
            .rposition(|message| matches!(message, Message::User(_)))
            .unwrap_or(0);
        let check_turn = matches!(
            request.messages.get(last_user),
            Some(Message::User(user)) if user.text_content() == CHECK_PROMPT
        );
        let results_this_turn = request.messages[last_user..]
            .iter()
            .filter(|message| matches!(message, Message::ToolResults { .. }))
            .count();
        let phase = self.phase.load(std::sync::atomic::Ordering::SeqCst);
        let step = match phase {
            PHASE_SINGLE_TURN => tool_results.len(),
            PHASE_FORK_ONLY if tool_results.is_empty() => 0,
            PHASE_FORK_ONLY => usize::MAX,
            _ if check_turn => results_this_turn + 1,
            _ => usize::MAX,
        };
        let calls: Vec<(&str, &str, serde_json::Value)> = if is_child {
            Vec::new()
        } else {
            match step {
                0 => vec![(
                    "call-fork",
                    "fork_off",
                    serde_json::json!({"member_id": CHILD, "task": FORK_TASK}),
                )],
                1 => vec![(
                    "call-check",
                    "mob_check_member",
                    serde_json::json!({"mob_id": &*self.mob_id, "member_id": CHILD}),
                )],
                2 => vec![(
                    "call-retire",
                    "mob_retire_member",
                    serde_json::json!({"mob_id": &*self.mob_id, "member_id": CHILD}),
                )],
                _ => Vec::new(),
            }
        };
        let stop_reason = if calls.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, stop_reason);
        let text = if is_child {
            "child done"
        } else {
            "forker done"
        };
        Box::pin(async_stream::stream! {
            if calls.is_empty() {
                yield Ok(LlmEvent::TextDelta { delta: text.to_string(), meta: None });
            }
            for (id, name, args) in calls {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: id.to_string(),
                    name: name.to_string(),
                    args,
                    meta: None,
                });
            }
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

async fn boot(
    state_path: &std::path::Path,
    script: &ForkerScript,
    runtime_options: meerkat_meta::RuntimeOptions,
) -> (
    meerkat_meta::UnifiedRuntime,
    Arc<dyn SessionBridge>,
    meerkat_meta::identity_first::AgentRuntimeId,
) {
    // The gateway's persistent composition: persistent mob storage (the
    // library builder keeps mob state in memory), so raw members such as a
    // fork child survive a restart.
    std::fs::create_dir_all(state_path).expect("state dir");
    let (mob_storage, provenance) = meerkat_meta::mob_composition_manifest::persistent_mob_storage(
        state_path.join("mob.sqlite3"),
    )
    .expect("mob storage");
    let spec = meerkat_meta::MobBootstrapSpec::persistent(
        MobDefinition::from_toml(&mob_toml(&script.mob_id)).expect("mob definition"),
        mob_storage,
        provenance,
        state_path.to_path_buf(),
        16,
    )
    .await
    .expect("persistent spec")
    // Creates no agent child mob (a fork child is a member of this mob). The
    // restart test boots twice in this process, which the durable child-mob
    // default refuses as a duplicate owner of the child root; explicit
    // ProcessBound keeps its in-process restart oracle.
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(script.clone())),
    });
    let unified = UnifiedRuntimeBuilder::default()
        .mob_spec(spec)
        .module_config(meerkat_meta::MetaConfig {
            modules: Vec::new(),
            discovery: meerkat_meta::DiscoverySpec {
                namespace: script.mob_id.to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        })
        .timeout(Duration::from_secs(10))
        .runtime_options(runtime_options)
        .build()
        .await
        .expect("build UnifiedRuntime");
    let session_service = unified
        .mob_runtime()
        .session_service()
        .cloned()
        .expect("session service");
    let bridge: Arc<dyn SessionBridge> = Arc::new(
        MobSessionBridge::with_session_service(unified.mob_handle(), session_service)
            .with_actor_admission_budget(Duration::from_mins(1)),
    );
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
    );
    let identity_rt = IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: script.mob_id.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    });
    let calendar = AgentIdentity::parse("domain:calendar").unwrap();
    let spec = DurableAgentSpec {
        identity: calendar.clone(),
        profile: ProfileName::from("domain"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let result = restore_flow(
        &identity_rt,
        &[spec],
        Some(&EmptyTopology as &dyn TopologyProvider),
        Some(&NoopCustomizer as &dyn AgentCustomizer),
    )
    .await
    .expect("restore_flow");
    let runtime_id = match result.outcomes.get(&calendar).expect("calendar outcome") {
        RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. } => {
            binding.agent_runtime_id.clone()
        }
        other => panic!("calendar was not created: {other:?}"),
    };
    (unified, bridge, runtime_id)
}

async fn deliver(
    bridge: &Arc<dyn SessionBridge>,
    runtime_id: &meerkat_meta::identity_first::AgentRuntimeId,
    text: &str,
) {
    bridge
        .deliver_awaiting_commit_with_mode_context_and_system_prompt(
            runtime_id,
            &meerkat_core::ContentInput::Text(text.to_string()),
            None,
            &[],
            HandlingMode::Queue,
            None,
        )
        .await
        .expect("the forker's turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forker_checks_and_retires_its_own_fork_child() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let script = ForkerScript::for_test("a_forker_checks_and_retires_its_own_fork_child");
    let (unified, bridge, runtime_id) = boot(
        &state_path,
        &script,
        meerkat_meta::RuntimeOptions::default(),
    )
    .await;
    deliver(&bridge, &runtime_id, "fork, check and retire").await;

    let (check, retire) = check_and_retire_results(&script);
    assert_eq!(
        tool_error(&check, "call-check"),
        None,
        "the forker may check its own fork child: {check}"
    );
    assert_eq!(
        tool_error(&retire, "call-retire"),
        None,
        "the forker may retire its own fork child: {retire}"
    );
    unified.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forker_checks_and_retires_its_idle_fork_child_after_a_restart() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let script =
        ForkerScript::for_test("a_forker_checks_and_retires_its_idle_fork_child_after_a_restart");
    script
        .phase
        .store(PHASE_FORK_ONLY, std::sync::atomic::Ordering::SeqCst);
    let (unified, bridge, runtime_id) = boot(
        &state_path,
        &script,
        meerkat_meta::RuntimeOptions::default(),
    )
    .await;
    deliver(&bridge, &runtime_id, "fork").await;
    let child = meerkat_mob::AgentIdentity::from(CHILD);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let status = unified.mob_handle().member_status(&child).await;
        let idle = status.as_ref().is_ok_and(|status| {
            status.progress.as_ref().is_some_and(|progress| {
                matches!(progress.run_state, meerkat_mob::MemberRunState::Idle)
            })
        });
        if idle {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never went idle: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    unified.shutdown().await;
    drop(unified);
    drop(bridge);

    script
        .phase
        .store(PHASE_CHECK_AND_RETIRE, std::sync::atomic::Ordering::SeqCst);
    let (unified, bridge, runtime_id) = boot(
        &state_path,
        &script,
        meerkat_meta::RuntimeOptions::default(),
    )
    .await;
    let restored = unified
        .mob_handle()
        .get_member(&child)
        .await
        .expect("roster read")
        .expect("the fork child survives the restart");
    assert_eq!(
        restored.spawned_by,
        Some(meerkat_mob::AgentIdentity::from("mk--domain_ccalendar")),
        "the restored child still names calendar's roster identity as its spawner"
    );
    deliver(&bridge, &runtime_id, CHECK_PROMPT).await;
    let (check, retire) = check_and_retire_results(&script);
    assert_eq!(
        tool_error(&check, "call-check"),
        None,
        "the forker may check its own fork child after a restart: {check}"
    );
    assert_eq!(
        tool_error(&retire, "call-retire"),
        None,
        "the forker may retire its own fork child after a restart: {retire}"
    );
    unified.shutdown().await;
}

/// The forker's `mob_check_member` and `mob_retire_member` results.
fn check_and_retire_results(script: &ForkerScript) -> (String, String) {
    let results = script.forker_tool_results.lock().unwrap().clone();
    let find = |call: &str| {
        results
            .iter()
            .find(|result| result.contains(call))
            .unwrap_or_else(|| panic!("{call} ran: {results:?}"))
            .clone()
    };
    (find("call-check"), find("call-retire"))
}

/// The typed tool error a recorded tool-results message carries for `call`:
/// `(error, data.kind)` from meerkat's canonical transcript payload, or `None`
/// when the call succeeded.
fn tool_error(recorded: &str, call: &str) -> Option<(String, Option<String>)> {
    let message: Message = serde_json::from_str(recorded).expect("recorded tool results");
    let Message::ToolResults { results, .. } = message else {
        panic!("recorded a non tool-results message: {recorded}");
    };
    let result = results
        .iter()
        .find(|result| result.tool_use_id == call)
        .unwrap_or_else(|| panic!("{call} has a result: {recorded}"));
    if !result.is_error {
        return None;
    }
    let payload: serde_json::Value =
        serde_json::from_str(&result.text_content()).expect("typed tool error payload");
    Some((
        payload["error"].as_str().unwrap_or_default().to_string(),
        payload["data"]["kind"].as_str().map(ToString::to_string),
    ))
}

/// The downstream app's case: the fork child was idle-retired (the default is 300 s,
/// here 1 s) before the forker checked and retired it. Meerkat #1234 observes
/// the target's presence before ownership, so both tools answer the typed
/// `member_retired` tool error, never access_denied.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_retired_fork_child_reads_as_typed_member_retired() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let script = ForkerScript::for_test("an_idle_retired_fork_child_reads_as_typed_member_retired");
    script
        .phase
        .store(PHASE_FORK_ONLY, std::sync::atomic::Ordering::SeqCst);
    let options = meerkat_meta::RuntimeOptions {
        implicit_delegate_idle_retire_secs: Some(1),
        implicit_delegate_idle_sweep_interval_ms: 100,
        ..meerkat_meta::RuntimeOptions::default()
    };
    let (unified, bridge, runtime_id) = boot(&state_path, &script, options).await;
    deliver(&bridge, &runtime_id, "fork").await;
    let child = meerkat_mob::AgentIdentity::from(CHILD);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while unified
        .mob_handle()
        .get_member(&child)
        .await
        .expect("roster read")
        .is_some()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the idle sweep never retired the fork child"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    script
        .phase
        .store(PHASE_CHECK_AND_RETIRE, std::sync::atomic::Ordering::SeqCst);
    deliver(&bridge, &runtime_id, CHECK_PROMPT).await;
    let (check, retire) = check_and_retire_results(&script);
    for (tool, recorded, call) in [
        ("mob_check_member", &check, "call-check"),
        ("mob_retire_member", &retire, "call-retire"),
    ] {
        assert_eq!(
            tool_error(recorded, call),
            Some((
                "execution_failed".to_string(),
                Some("member_retired".to_string())
            )),
            "a retired child's {tool} answers the typed member_retired error: {recorded}"
        );
    }
    unified.shutdown().await;
}
