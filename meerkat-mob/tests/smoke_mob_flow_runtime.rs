#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//!
//! Live E2E smoke tests for the public flow runtime surface.
//!
//! These tests drive `MobHandle::run_flow(...)` through real API-backed members
//! and focus on frame-root flows with loops, sibling parallelism, and fanout.
//!
//! Run with:
//!   cargo test -p meerkat-mob --test smoke_mob_flow_runtime \
//!     --features integration-real-tests -- --ignored --nocapture

use indexmap::IndexMap;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::definition::{
    BackendConfig, CollectionPolicy, ConditionExpr, DependencyMode, DispatchMode, FlowNodeSpec,
    FlowSpec, FlowStepSpec, FrameSpec, FrameStepSpec, LimitsSpec, OrchestratorConfig,
    RepeatUntilSpec, WiringRules,
};
use meerkat_mob::{
    AgentIdentity, FlowId, LoopId, MobBuilder, MobDefinition, MobHandle, MobId, MobRun,
    MobRunStatus, MobRuntimeMode, MobSessionService, MobStorage, Profile, ProfileBinding,
    ProfileName, SpawnMemberSpec, StepId, ToolConfig,
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, MemoryBlobStore, StoreAdapter};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tempfile::TempDir;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Duration, Instant, sleep};

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn flow_runtime_smoke_concurrency_from_env(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
}

fn flow_runtime_smoke_semaphore() -> &'static Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE.get_or_init(|| {
        Arc::new(Semaphore::new(flow_runtime_smoke_concurrency_from_env(
            std::env::var("MEERKAT_FLOW_RUNTIME_SMOKE_CONCURRENCY")
                .ok()
                .as_deref(),
        )))
    })
}

async fn acquire_flow_runtime_smoke_permit(test_name: &str) -> OwnedSemaphorePermit {
    let available_before = flow_runtime_smoke_semaphore().available_permits();
    eprintln!(
        "Waiting for flow runtime smoke concurrency permit for {test_name} (available={available_before})"
    );
    let permit = flow_runtime_smoke_semaphore()
        .clone()
        .acquire_owned()
        .await
        .expect("flow runtime smoke semaphore should stay open");
    eprintln!("Acquired flow runtime smoke concurrency permit for {test_name}");
    permit
}

#[test]
fn flow_runtime_smoke_concurrency_defaults_to_bounded_parallelism() {
    assert_eq!(flow_runtime_smoke_concurrency_from_env(None), 4);
    assert_eq!(flow_runtime_smoke_concurrency_from_env(Some("0")), 4);
    assert_eq!(
        flow_runtime_smoke_concurrency_from_env(Some("not-a-number")),
        4
    );
    assert_eq!(flow_runtime_smoke_concurrency_from_env(Some("8")), 8);
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

#[derive(Clone, Debug)]
struct FlowSmokeModels {
    lead: String,
    worker: String,
    reviewer: String,
    analyst: String,
}

fn explicit_flow_smoke_model() -> Option<String> {
    std::env::var("FLOW_SMOKE_MODEL")
        .or_else(|_| std::env::var("SMOKE_MODEL"))
        .ok()
}

fn has_key_for_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude-") {
        anthropic_api_key().is_some()
    } else if lower.starts_with("gpt-") || lower.starts_with("chatgpt-") {
        openai_api_key().is_some()
    } else if lower.starts_with("gemini-") {
        gemini_api_key().is_some()
    } else {
        anthropic_api_key().is_some() || openai_api_key().is_some() || gemini_api_key().is_some()
    }
}

fn flow_smoke_models() -> Option<FlowSmokeModels> {
    if let Some(model) = explicit_flow_smoke_model() {
        if has_key_for_model(&model) {
            return Some(FlowSmokeModels {
                lead: model.clone(),
                worker: model.clone(),
                reviewer: model.clone(),
                analyst: model,
            });
        }
        return None;
    }

    match (anthropic_api_key().is_some(), gemini_api_key().is_some()) {
        (true, true) => Some(FlowSmokeModels {
            lead: "claude-haiku-4-5-20251001".to_string(),
            worker: "claude-haiku-4-5-20251001".to_string(),
            reviewer: "claude-haiku-4-5-20251001".to_string(),
            analyst: "gemini-3.1-flash-lite-preview".to_string(),
        }),
        (true, false) => Some(FlowSmokeModels {
            lead: "claude-haiku-4-5-20251001".to_string(),
            worker: "claude-haiku-4-5-20251001".to_string(),
            reviewer: "claude-haiku-4-5-20251001".to_string(),
            analyst: "claude-haiku-4-5-20251001".to_string(),
        }),
        (false, true) => Some(FlowSmokeModels {
            lead: "gemini-3.1-flash-lite-preview".to_string(),
            worker: "gemini-3.1-flash-lite-preview".to_string(),
            reviewer: "gemini-3.1-flash-lite-preview".to_string(),
            analyst: "gemini-3.1-flash-lite-preview".to_string(),
        }),
        (false, false) => None,
    }
}

fn flow_smoke_models_label(models: &FlowSmokeModels) -> String {
    format!(
        "lead={}, worker={}, reviewer={}, analyst={}",
        models.lead, models.worker, models.reviewer, models.analyst
    )
}

#[derive(Clone)]
struct FlowSmokePaths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
    sessions_root: PathBuf,
    mob_db_path: PathBuf,
}

impl FlowSmokePaths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
            sessions_root: root.join("sessions-jsonl"),
            mob_db_path: root.join("mob.db"),
        }
    }
}

fn smoke_factory(paths: &FlowSmokePaths) -> AgentFactory {
    AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .comms(true)
}

fn persistent_service(
    paths: &FlowSmokePaths,
) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
    let factory = smoke_factory(paths);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    let store = Arc::new(JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::default());
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    Arc::new(PersistentSessionService::new(
        builder,
        32,
        store_dyn,
        Some(runtime_store),
        blob_store,
    ))
}

fn flow_profile(model: &str, peer_description: &str) -> Profile {
    Profile {
        model: model.to_string(),
        skills: vec![],
        tools: ToolConfig {
            comms: true,
            ..Default::default()
        },
        peer_description: peer_description.to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    }
}

fn json_step(role: &str, message: impl Into<String>) -> FlowStepSpec {
    FlowStepSpec {
        role: ProfileName::from(role),
        message: message.into().into(),
        depends_on: Vec::new(),
        dispatch_mode: DispatchMode::OneToOne,
        collection_policy: CollectionPolicy::Any,
        condition: None,
        timeout_ms: Some(120_000),
        expected_schema_ref: None,
        branch: None,
        depends_on_mode: DependencyMode::All,
        allowed_tools: None,
        blocked_tools: None,
        output_format: meerkat_mob::definition::StepOutputFormat::Json,
    }
}

fn exact_json_message(value: Value) -> String {
    format!(
        "You are a deterministic flow smoke-test responder.\n\
Return raw JSON only. No markdown.\n\
Return exactly this JSON object and nothing else:\n{}",
        serde_json::to_string(&value).expect("serialize deterministic JSON value")
    )
}

fn exact_json_step(role: &str, value: Value) -> FlowStepSpec {
    json_step(role, exact_json_message(value))
}

fn exact_json_step_with_condition(
    role: &str,
    value: Value,
    condition: ConditionExpr,
) -> FlowStepSpec {
    let mut step = exact_json_step(role, value);
    step.condition = Some(condition);
    step
}

fn with_branch(mut step: FlowStepSpec, branch: &str) -> FlowStepSpec {
    step.branch = Some(meerkat_mob::BranchId::from(branch));
    step
}

fn condition_eq(path: impl Into<String>, value: Value) -> ConditionExpr {
    ConditionExpr::Eq {
        path: path.into(),
        value,
    }
}

fn condition_not_eq(path: impl Into<String>, value: Value) -> ConditionExpr {
    ConditionExpr::Not {
        expr: Box::new(condition_eq(path, value)),
    }
}

fn frame_step(step_id: &str, depends_on: Vec<&str>) -> FlowNodeSpec {
    FlowNodeSpec::Step(FrameStepSpec {
        step_id: StepId::from(step_id),
        depends_on: depends_on
            .into_iter()
            .map(meerkat_mob::FlowNodeId::from)
            .collect(),
        depends_on_mode: DependencyMode::All,
        branch: None,
    })
}

fn repeat_until_node(
    loop_id: &str,
    depends_on: Vec<&str>,
    body: FrameSpec,
    until_path: &str,
    max_iterations: u32,
) -> FlowNodeSpec {
    FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
        loop_id: LoopId::from(loop_id),
        depends_on: depends_on
            .into_iter()
            .map(meerkat_mob::FlowNodeId::from)
            .collect(),
        depends_on_mode: DependencyMode::All,
        body,
        until: ConditionExpr::Eq {
            path: until_path.to_string(),
            value: serde_json::json!(true),
        },
        max_iterations,
    })
}

fn repeat_until_node_any(
    loop_id: &str,
    depends_on: Vec<&str>,
    body: FrameSpec,
    until_path: &str,
    max_iterations: u32,
) -> FlowNodeSpec {
    FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
        loop_id: LoopId::from(loop_id),
        depends_on: depends_on
            .into_iter()
            .map(meerkat_mob::FlowNodeId::from)
            .collect(),
        depends_on_mode: DependencyMode::Any,
        body,
        until: ConditionExpr::Eq {
            path: until_path.to_string(),
            value: serde_json::json!(true),
        },
        max_iterations,
    })
}

fn insert_two_phase_steps(
    steps: &mut IndexMap<StepId, FlowStepSpec>,
    loop_id: &str,
    first_step_id: &str,
    second_step_id: &str,
    role: &str,
    first_value: Value,
    second_value: Value,
) {
    let first_path = format!("loops.{loop_id}.iterations.0.steps.{first_step_id}.done");
    steps.insert(
        StepId::from(first_step_id),
        exact_json_step_with_condition(
            role,
            first_value,
            condition_not_eq(first_path.clone(), serde_json::json!(false)),
        ),
    );
    steps.insert(StepId::from(second_step_id), {
        let mut step = exact_json_step_with_condition(
            role,
            second_value,
            condition_eq(first_path, serde_json::json!(false)),
        );
        step.depends_on = vec![StepId::from(first_step_id)];
        step
    });
}

fn prompt_prior_toggle_step(
    role: &str,
    prior_expr: &str,
    first_value: Value,
    second_value: Value,
) -> FlowStepSpec {
    let first_json =
        serde_json::to_string(&first_value).expect("serialize first persisted-loop value");
    let second_json =
        serde_json::to_string(&second_value).expect("serialize second persisted-loop value");
    json_step(
        role,
        format!(
            "You are a deterministic loop-body responder.\n\
Return raw JSON only. No markdown.\n\
prior_value={{{{{prior_expr}}}}}\n\
If prior_value is null, return exactly:\n\
{first_json}\n\
Otherwise return exactly:\n\
{second_json}"
        ),
    )
}

fn flow_definition(models: &FlowSmokeModels) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(flow_profile(&models.lead, "Lead flow responder")),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(flow_profile(&models.worker, "Worker flow responder")),
    );
    profiles.insert(
        ProfileName::from("reviewer"),
        ProfileBinding::Inline(flow_profile(&models.reviewer, "Reviewer flow responder")),
    );
    profiles.insert(
        ProfileName::from("analyst"),
        ProfileBinding::Inline(flow_profile(&models.analyst, "Analyst flow responder")),
    );

    let mut flows = BTreeMap::new();
    flows.insert(
        FlowId::from("fanout_review_loop"),
        build_fanout_review_loop_flow(),
    );
    flows.insert(
        FlowId::from("dual_loops_join"),
        build_dual_loops_join_flow(),
    );
    flows.insert(
        FlowId::from("branch_then_review_loop"),
        build_branch_then_review_loop_flow(),
    );
    flows.insert(
        FlowId::from("parallel_body_siblings_join"),
        build_parallel_body_siblings_join_flow(),
    );
    flows.insert(
        FlowId::from("nested_outer_inner_loop"),
        build_nested_outer_inner_loop_flow(),
    );
    flows.insert(
        FlowId::from("fanin_after_parallel_loops"),
        build_fanin_after_parallel_loops_flow(),
    );
    flows.insert(
        FlowId::from("conditional_skip_inside_body_loop"),
        build_conditional_skip_inside_body_loop_flow(),
    );
    flows.insert(
        FlowId::from("three_way_branch_loop_audit"),
        build_three_way_branch_loop_audit_flow(),
    );
    flows.insert(
        FlowId::from("sequential_loop_chain"),
        build_sequential_loop_chain_flow(),
    );
    flows.insert(
        FlowId::from("three_sibling_loops_join"),
        build_three_sibling_loops_join_flow(),
    );
    flows.insert(
        FlowId::from("outer_branch_inner_loop"),
        build_outer_branch_inner_loop_flow(),
    );
    flows.insert(FlowId::from("maximal_matrix"), build_maximal_matrix_flow());
    flows.insert(
        FlowId::from("persisted_branch_parallel_review_loop"),
        build_persisted_branch_parallel_review_loop_flow(),
    );
    flows.insert(
        FlowId::from("persisted_dual_loops_fanin"),
        build_persisted_dual_loops_fanin_flow(),
    );
    flows.insert(
        FlowId::from("persisted_sequential_loop_chain"),
        build_persisted_sequential_loop_chain_flow(),
    );
    flows.insert(
        FlowId::from("persisted_three_sibling_loops_join"),
        build_persisted_three_sibling_loops_join_flow(),
    );
    flows.insert(
        FlowId::from("persisted_branch_dual_loops_audit"),
        build_persisted_branch_dual_loops_audit_flow(),
    );

    let mut definition = MobDefinition::explicit(MobId::from("flow-runtime-smoke"));
    definition.orchestrator = Some(OrchestratorConfig {
        profile: ProfileName::from("lead"),
    });
    definition.profiles = profiles;
    definition.wiring = WiringRules::default();
    definition.backend = BackendConfig::default();
    definition.flows = flows;
    definition.limits = Some(LimitsSpec {
        max_flow_duration_ms: Some(300_000),
        max_step_retries: Some(0),
        max_orphaned_turns: Some(8),
        cancel_grace_timeout_ms: Some(1_500),
        max_active_nodes: Some(4),
        max_active_frames: Some(4),
        max_frame_depth: Some(4),
    });
    definition
}

fn build_fanout_review_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();

    let mut gather = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown and no extra prose.
Topic={{params.topic}}
Return an object with exactly these keys:
- "topic": the exact topic string
- "source": a short label of 1-2 words
- "fact": one short observation about the topic
- "done": true"#,
    );
    gather.dispatch_mode = DispatchMode::FanOut;
    gather.collection_policy = CollectionPolicy::All;
    steps.insert(StepId::from("gather"), gather);

    steps.insert(
        StepId::from("stabilize"),
        json_step(
            "worker",
            r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Gathered inputs={{steps.gather}}
Return exactly:
{"summary":"gather stabilized","done":true}"#,
        ),
    );

    steps.insert(
        StepId::from("parallel_note"),
        json_step(
            "lead",
            r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly:
{"note":"parallel sibling complete","done":true}"#,
        ),
    );

    steps.insert(
        StepId::from("draft"),
        json_step(
            "worker",
            r#"You are a deterministic loop-body responder.
Return raw JSON only. No markdown.
topic={{params.topic}}
prior_review={{loops.review_loop.iterations.0.steps.review.approved}}
If prior_review is null, return exactly:
{"attempt":1,"draft":"first pass","needs_revision":true,"review_approved":false,"review_done":false,"review_summary":"needs one revision"}
Otherwise return exactly:
{"attempt":2,"draft":"revised pass","needs_revision":false,"review_approved":true,"review_done":true,"review_summary":"approved on second pass"}"#,
        ),
    );

    let mut review = json_step(
        "reviewer",
        r#"You are a deterministic review responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"approved": {{steps.draft.review_approved}}, "done": {{steps.draft.review_done}}, "summary": "{{steps.draft.review_summary}}"}"#,
    );
    review.depends_on = vec![StepId::from("draft")];
    steps.insert(StepId::from("review"), review);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
review_summary={{steps.review.summary}}
parallel_note={{steps.parallel_note.note}}
Return exactly:
{"final_message":"approved on second pass with parallel sibling complete","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("parallel_note"), StepId::from("review")];
    steps.insert(StepId::from("finalize"), finalize);

    let review_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("draft-node"),
                frame_step("draft", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("review-node"),
                frame_step("review", vec!["draft-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("gather-node"),
                frame_step("gather", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("stabilize-node"),
                frame_step("stabilize", vec!["gather-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("parallel-note-node"),
                frame_step("parallel_note", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("review-loop-node"),
                repeat_until_node(
                    "review_loop",
                    vec!["stabilize-node", "parallel-note-node"],
                    review_body,
                    "steps.review.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-node"),
                frame_step("finalize", vec!["review-loop-node", "parallel-note-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("fanout + sibling + review loop smoke".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_dual_loops_join_flow() -> FlowSpec {
    let mut steps = IndexMap::new();

    steps.insert(
        StepId::from("left"),
        json_step(
            "worker",
            r#"You are a deterministic loop-body responder.
Return raw JSON only. No markdown.
prior_done={{loops.left_loop.iterations.0.steps.left.done}}
If prior_done is null, return exactly:
{"done":false,"attempt":1,"path":"left-first"}
Otherwise return exactly:
{"done":true,"attempt":2,"path":"left-second"}"#,
        ),
    );
    steps.insert(
        StepId::from("right"),
        json_step(
            "reviewer",
            r#"You are a deterministic loop-body responder.
Return raw JSON only. No markdown.
prior_done={{loops.right_loop.iterations.0.steps.right.done}}
If prior_done is null, return exactly:
{"done":false,"attempt":1,"path":"right-first"}
Otherwise return exactly:
{"done":true,"attempt":2,"path":"right-second"}"#,
        ),
    );
    let mut join = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
left_path={{steps.left.path}}
right_path={{steps.right.path}}
Return exactly:
{"final_message":"joined left-second and right-second","done":true}"#,
    );
    join.depends_on = vec![StepId::from("left"), StepId::from("right")];
    steps.insert(StepId::from("join"), join);

    let left_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("left-body-node"),
            frame_step("left", vec![]),
        )]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("right-body-node"),
            frame_step("right", vec![]),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("left-loop-node"),
                repeat_until_node("left_loop", vec![], left_body, "steps.left.done", 3),
            ),
            (
                meerkat_mob::FlowNodeId::from("right-loop-node"),
                repeat_until_node("right_loop", vec![], right_body, "steps.right.done", 3),
            ),
            (
                meerkat_mob::FlowNodeId::from("join-node"),
                frame_step("join", vec!["left-loop-node", "right-loop-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("dual sibling loops with downstream join smoke".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_branch_then_review_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("critical_route"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"critical","done":true}),
                condition_eq("params.severity", serde_json::json!("critical")),
            ),
            "severity_route",
        ),
    );
    steps.insert(
        StepId::from("minor_route"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"minor","done":true}),
                condition_eq("params.severity", serde_json::json!("minor")),
            ),
            "severity_route",
        ),
    );
    insert_two_phase_steps(
        &mut steps,
        "branch_review_loop",
        "revise_first",
        "revise_second",
        "reviewer",
        serde_json::json!({"done":false,"stage":"revise-first"}),
        serde_json::json!({"done":true,"stage":"revise-second"}),
    );
    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"selected":"{{steps.minor_route.route}}","revision":"{{steps.revise_second.stage}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("minor_route"), StepId::from("revise_second")];
    steps.insert(StepId::from("finalize_branch_loop"), finalize);

    let review_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("revise-first-node"),
                frame_step("revise_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("revise-second-node"),
                frame_step("revise_second", vec!["revise-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("critical-route-node"),
                frame_step("critical_route", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("minor-route-node"),
                frame_step("minor_route", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("branch-review-loop-node"),
                repeat_until_node_any(
                    "branch_review_loop",
                    vec!["critical-route-node", "minor-route-node"],
                    review_body,
                    "steps.revise_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-branch-loop-node"),
                frame_step("finalize_branch_loop", vec!["branch-review-loop-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("branch winner feeding a two-pass review loop".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_parallel_body_siblings_join_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "parallel_body_loop",
        "left_first",
        "left_second",
        "worker",
        serde_json::json!({"done":false,"left":"L1"}),
        serde_json::json!({"done":true,"left":"L2"}),
    );
    insert_two_phase_steps(
        &mut steps,
        "parallel_body_loop",
        "right_first",
        "right_second",
        "reviewer",
        serde_json::json!({"done":false,"right":"R1"}),
        serde_json::json!({"done":true,"right":"R2"}),
    );

    let mut body_join = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"done":true,"tag":"join-final","left":"{{steps.left_second.left}}","right":"{{steps.right_second.right}}"}"#,
    );
    body_join.depends_on = vec![StepId::from("left_second"), StepId::from("right_second")];
    steps.insert(StepId::from("body_join"), body_join);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"left":"{{steps.left_second.left}}","right":"{{steps.right_second.right}}","join":"{{steps.body_join.tag}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("left_second"),
        StepId::from("right_second"),
        StepId::from("body_join"),
    ];
    steps.insert(StepId::from("finalize_parallel_body"), finalize);

    let body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("left-first-node"),
                frame_step("left_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("right-first-node"),
                frame_step("right_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("left-second-node"),
                frame_step("left_second", vec!["left-first-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("right-second-node"),
                frame_step("right_second", vec!["right-first-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("body-join-node"),
                frame_step("body_join", vec!["left-second-node", "right-second-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("parallel-body-loop-node"),
                repeat_until_node(
                    "parallel_body_loop",
                    vec![],
                    body,
                    "steps.body_join.done",
                    2,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-parallel-body-node"),
                frame_step("finalize_parallel_body", vec!["parallel-body-loop-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("parallel loop-body siblings with per-iteration join".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_nested_outer_inner_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "inner_loop",
        "inner_first",
        "inner_second",
        "worker",
        serde_json::json!({"done":false,"inner":"inner-1"}),
        serde_json::json!({"done":true,"inner":"inner-2"}),
    );
    insert_two_phase_steps(
        &mut steps,
        "outer_loop",
        "outer_first",
        "outer_second",
        "lead",
        serde_json::json!({"done":false,"outer":"outer-1"}),
        serde_json::json!({"done":true,"outer":"outer-2"}),
    );
    steps
        .get_mut(&StepId::from("outer_first"))
        .expect("outer_first")
        .depends_on
        .push(StepId::from("inner_second"));
    steps
        .get_mut(&StepId::from("outer_second"))
        .expect("outer_second")
        .depends_on
        .push(StepId::from("inner_second"));

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"outer":"{{steps.outer_second.outer}}","inner":"{{steps.inner_second.inner}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("outer_second"), StepId::from("inner_second")];
    steps.insert(StepId::from("finalize_nested_loop"), finalize);

    let inner_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("inner-first-node"),
                frame_step("inner_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("inner-second-node"),
                frame_step("inner_second", vec!["inner-first-node"]),
            ),
        ]),
    };

    let outer_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("inner-loop-node"),
                repeat_until_node(
                    "inner_loop",
                    vec![],
                    inner_body,
                    "steps.inner_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("outer-first-node"),
                frame_step("outer_first", vec!["inner-loop-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("outer-second-node"),
                frame_step("outer_second", vec!["outer-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("outer-loop-node"),
                repeat_until_node(
                    "outer_loop",
                    vec![],
                    outer_body,
                    "steps.outer_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-nested-loop-node"),
                frame_step("finalize_nested_loop", vec!["outer-loop-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("nested outer/inner loop smoke".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_fanin_after_parallel_loops_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "fanin_left_loop",
        "fanin_left_first",
        "fanin_left_second",
        "worker",
        serde_json::json!({"done":false,"left":"left-1"}),
        serde_json::json!({"done":true,"left":"left-2"}),
    );
    insert_two_phase_steps(
        &mut steps,
        "fanin_right_loop",
        "fanin_right_first",
        "fanin_right_second",
        "reviewer",
        serde_json::json!({"done":false,"right":"right-1"}),
        serde_json::json!({"done":true,"right":"right-2"}),
    );

    let mut audit = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"audit","left":"{{steps.fanin_left_second.left}}","right":"{{steps.fanin_right_second.right}}","done":true}"#,
    );
    audit.dispatch_mode = DispatchMode::FanIn;
    audit.collection_policy = CollectionPolicy::All;
    audit.depends_on = vec![
        StepId::from("fanin_left_second"),
        StepId::from("fanin_right_second"),
    ];
    steps.insert(StepId::from("fanin_audit"), audit);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"{{steps.fanin_audit.0.output.kind}}","left":"{{steps.fanin_audit.0.output.left}}","right":"{{steps.fanin_audit.0.output.right}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("fanin_audit")];
    steps.insert(StepId::from("finalize_fanin"), finalize);

    let left_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("fanin-left-first-node"),
                frame_step("fanin_left_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("fanin-left-second-node"),
                frame_step("fanin_left_second", vec!["fanin-left-first-node"]),
            ),
        ]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("fanin-right-first-node"),
                frame_step("fanin_right_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("fanin-right-second-node"),
                frame_step("fanin_right_second", vec!["fanin-right-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("fanin-left-loop-node"),
                repeat_until_node(
                    "fanin_left_loop",
                    vec![],
                    left_body,
                    "steps.fanin_left_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("fanin-right-loop-node"),
                repeat_until_node(
                    "fanin_right_loop",
                    vec![],
                    right_body,
                    "steps.fanin_right_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("fanin-audit-node"),
                frame_step(
                    "fanin_audit",
                    vec!["fanin-left-loop-node", "fanin-right-loop-node"],
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-fanin-node"),
                frame_step("finalize_fanin", vec!["fanin-audit-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("parallel loops feeding a fan-in aggregate".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_conditional_skip_inside_body_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "conditional_body_loop",
        "tick_first",
        "tick_second",
        "worker",
        serde_json::json!({"done":false,"phase":"phase-1"}),
        serde_json::json!({"done":true,"phase":"phase-2"}),
    );
    steps.insert(
        StepId::from("optional_path"),
        exact_json_step_with_condition(
            "reviewer",
            serde_json::json!({"done":true,"path":"optional"}),
            condition_eq("params.mode", serde_json::json!("optional")),
        ),
    );
    steps.insert(
        StepId::from("fallback_path"),
        exact_json_step_with_condition(
            "reviewer",
            serde_json::json!({"done":true,"path":"fallback"}),
            condition_eq("params.mode", serde_json::json!("fallback")),
        ),
    );

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"path":"{{steps.fallback_path.path}}","phase":"{{steps.tick_second.phase}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("fallback_path"), StepId::from("tick_second")];
    steps.insert(StepId::from("finalize_conditional_body"), finalize);

    let body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("tick-first-node"),
                frame_step("tick_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("tick-second-node"),
                frame_step("tick_second", vec!["tick-first-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("optional-path-node"),
                frame_step("optional_path", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("fallback-path-node"),
                frame_step("fallback_path", vec![]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("conditional-body-loop-node"),
                repeat_until_node(
                    "conditional_body_loop",
                    vec![],
                    body,
                    "steps.tick_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-conditional-body-node"),
                frame_step(
                    "finalize_conditional_body",
                    vec!["conditional-body-loop-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("conditional skip behavior inside a body loop".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_three_way_branch_loop_audit_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("alpha_route"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"alpha","done":true}),
                condition_eq("params.route", serde_json::json!("alpha")),
            ),
            "branch_pick",
        ),
    );
    steps.insert(
        StepId::from("beta_route"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"beta","done":true}),
                condition_eq("params.route", serde_json::json!("beta")),
            ),
            "branch_pick",
        ),
    );
    steps.insert(
        StepId::from("gamma_route"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"gamma","done":true}),
                condition_eq("params.route", serde_json::json!("gamma")),
            ),
            "branch_pick",
        ),
    );
    insert_two_phase_steps(
        &mut steps,
        "branch_audit_loop",
        "audit_first",
        "audit_second",
        "reviewer",
        serde_json::json!({"done":false,"phase":"audit-1"}),
        serde_json::json!({"done":true,"phase":"audit-2"}),
    );

    let mut branch_audit = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"branch-audit","route":"{{steps.gamma_route.route}}","phase":"{{steps.audit_second.phase}}","done":true}"#,
    );
    branch_audit.dispatch_mode = DispatchMode::FanOut;
    branch_audit.collection_policy = CollectionPolicy::All;
    branch_audit.depends_on = vec![StepId::from("gamma_route"), StepId::from("audit_second")];
    steps.insert(StepId::from("branch_audit"), branch_audit);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"route":"{{steps.gamma_route.route}}","phase":"{{steps.audit_second.phase}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("branch_audit")];
    steps.insert(StepId::from("finalize_three_way_branch"), finalize);

    let body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("audit-first-node"),
                frame_step("audit_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("audit-second-node"),
                frame_step("audit_second", vec!["audit-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("alpha-route-node"),
                frame_step("alpha_route", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("beta-route-node"),
                frame_step("beta_route", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("gamma-route-node"),
                frame_step("gamma_route", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("branch-audit-loop-node"),
                repeat_until_node_any(
                    "branch_audit_loop",
                    vec!["alpha-route-node", "beta-route-node", "gamma-route-node"],
                    body,
                    "steps.audit_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("branch-audit-node"),
                frame_step("branch_audit", vec!["branch-audit-loop-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-three-way-branch-node"),
                frame_step("finalize_three_way_branch", vec!["branch-audit-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("three-way branch feeding a review loop and analyst audit".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_sequential_loop_chain_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "chain_left_loop",
        "chain_left_first",
        "chain_left_second",
        "worker",
        serde_json::json!({"done":false,"left":"left-1"}),
        serde_json::json!({"done":true,"left":"left-2"}),
    );
    steps.insert(
        StepId::from("chain_bridge"),
        exact_json_step("lead", serde_json::json!({"bridge":"handoff","done":true})),
    );
    insert_two_phase_steps(
        &mut steps,
        "chain_right_loop",
        "chain_right_first",
        "chain_right_second",
        "reviewer",
        serde_json::json!({"done":false,"right":"right-1"}),
        serde_json::json!({"done":true,"right":"right-2"}),
    );

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"left":"{{steps.chain_left_second.left}}","bridge":"{{steps.chain_bridge.bridge}}","right":"{{steps.chain_right_second.right}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("chain_left_second"),
        StepId::from("chain_bridge"),
        StepId::from("chain_right_second"),
    ];
    steps.insert(StepId::from("finalize_sequential_chain"), finalize);

    let left_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("chain-left-first-node"),
                frame_step("chain_left_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("chain-left-second-node"),
                frame_step("chain_left_second", vec!["chain-left-first-node"]),
            ),
        ]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("chain-right-first-node"),
                frame_step("chain_right_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("chain-right-second-node"),
                frame_step("chain_right_second", vec!["chain-right-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("chain-left-loop-node"),
                repeat_until_node(
                    "chain_left_loop",
                    vec![],
                    left_body,
                    "steps.chain_left_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("chain-bridge-node"),
                frame_step("chain_bridge", vec!["chain-left-loop-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("chain-right-loop-node"),
                repeat_until_node(
                    "chain_right_loop",
                    vec!["chain-bridge-node"],
                    right_body,
                    "steps.chain_right_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-sequential-chain-node"),
                frame_step("finalize_sequential_chain", vec!["chain-right-loop-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some("one loop feeding a bridge that feeds a second loop".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_three_sibling_loops_join_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    insert_two_phase_steps(
        &mut steps,
        "alpha_loop",
        "alpha_first",
        "alpha_second",
        "worker",
        serde_json::json!({"done":false,"alpha":"A1"}),
        serde_json::json!({"done":true,"alpha":"A2"}),
    );
    insert_two_phase_steps(
        &mut steps,
        "beta_loop",
        "beta_first",
        "beta_second",
        "reviewer",
        serde_json::json!({"done":false,"beta":"B1"}),
        serde_json::json!({"done":true,"beta":"B2"}),
    );
    insert_two_phase_steps(
        &mut steps,
        "gamma_loop",
        "gamma_first",
        "gamma_second",
        "lead",
        serde_json::json!({"done":false,"gamma":"G1"}),
        serde_json::json!({"done":true,"gamma":"G2"}),
    );

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"alpha":"{{steps.alpha_second.alpha}}","beta":"{{steps.beta_second.beta}}","gamma":"{{steps.gamma_second.gamma}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("alpha_second"),
        StepId::from("beta_second"),
        StepId::from("gamma_second"),
    ];
    steps.insert(StepId::from("finalize_three_sibling_loops"), finalize);

    let alpha_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("alpha-first-node"),
                frame_step("alpha_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("alpha-second-node"),
                frame_step("alpha_second", vec!["alpha-first-node"]),
            ),
        ]),
    };
    let beta_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("beta-first-node"),
                frame_step("beta_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("beta-second-node"),
                frame_step("beta_second", vec!["beta-first-node"]),
            ),
        ]),
    };
    let gamma_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("gamma-first-node"),
                frame_step("gamma_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("gamma-second-node"),
                frame_step("gamma_second", vec!["gamma-first-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("alpha-loop-node"),
                repeat_until_node(
                    "alpha_loop",
                    vec![],
                    alpha_body,
                    "steps.alpha_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("beta-loop-node"),
                repeat_until_node("beta_loop", vec![], beta_body, "steps.beta_second.done", 3),
            ),
            (
                meerkat_mob::FlowNodeId::from("gamma-loop-node"),
                repeat_until_node(
                    "gamma_loop",
                    vec![],
                    gamma_body,
                    "steps.gamma_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-three-sibling-loops-node"),
                frame_step(
                    "finalize_three_sibling_loops",
                    vec!["alpha-loop-node", "beta-loop-node", "gamma-loop-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("three sibling loops converging in a downstream join".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_outer_branch_inner_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("route_red"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"red","done":true}),
                condition_eq("params.route", serde_json::json!("red")),
            ),
            "outer_route",
        ),
    );
    steps.insert(
        StepId::from("route_blue"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"blue","done":true}),
                condition_eq("params.route", serde_json::json!("blue")),
            ),
            "outer_route",
        ),
    );
    insert_two_phase_steps(
        &mut steps,
        "branch_inner_loop",
        "branch_inner_first",
        "branch_inner_second",
        "reviewer",
        serde_json::json!({"done":false,"inner":"inner-1"}),
        serde_json::json!({"done":true,"inner":"inner-2"}),
    );
    steps.insert(
        StepId::from("outer_branch_gate"),
        exact_json_step(
            "lead",
            serde_json::json!({"outer":"outer-branch","done":true}),
        ),
    );
    steps
        .get_mut(&StepId::from("outer_branch_gate"))
        .expect("outer_branch_gate")
        .depends_on
        .push(StepId::from("branch_inner_second"));

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"route":"{{steps.route_blue.route}}","outer":"{{steps.outer_branch_gate.outer}}","inner":"{{steps.branch_inner_second.inner}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("route_blue"),
        StepId::from("outer_branch_gate"),
        StepId::from("branch_inner_second"),
    ];
    steps.insert(StepId::from("finalize_outer_branch_inner"), finalize);

    let inner_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("branch-inner-first-node"),
                frame_step("branch_inner_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("branch-inner-second-node"),
                frame_step("branch_inner_second", vec!["branch-inner-first-node"]),
            ),
        ]),
    };
    let outer_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("branch-inner-loop-node"),
                repeat_until_node(
                    "branch_inner_loop",
                    vec![],
                    inner_body,
                    "steps.branch_inner_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("outer-branch-gate-node"),
                frame_step("outer_branch_gate", vec!["branch-inner-loop-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("route-red-node"),
                frame_step("route_red", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("route-blue-node"),
                frame_step("route_blue", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("outer-branch-loop-node"),
                repeat_until_node_any(
                    "outer_branch_loop",
                    vec!["route-red-node", "route-blue-node"],
                    outer_body,
                    "steps.outer_branch_gate.done",
                    2,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-outer-branch-inner-node"),
                frame_step(
                    "finalize_outer_branch_inner",
                    vec!["outer-branch-loop-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("root branch feeding an outer loop that hosts an inner loop".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_maximal_matrix_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("matrix_primary"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"primary","done":true}),
                condition_eq("params.route", serde_json::json!("primary")),
            ),
            "matrix_route",
        ),
    );
    steps.insert(
        StepId::from("matrix_secondary"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"secondary","done":true}),
                condition_eq("params.route", serde_json::json!("secondary")),
            ),
            "matrix_route",
        ),
    );

    let mut matrix_gather = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"matrix-gather","done":true}"#,
    );
    matrix_gather.dispatch_mode = DispatchMode::FanOut;
    matrix_gather.collection_policy = CollectionPolicy::All;
    steps.insert(StepId::from("matrix_gather"), matrix_gather);

    steps.insert(
        StepId::from("matrix_parallel_note"),
        exact_json_step(
            "lead",
            serde_json::json!({"note":"parallel-note","done":true}),
        ),
    );
    insert_two_phase_steps(
        &mut steps,
        "matrix_inner_loop",
        "matrix_inner_first",
        "matrix_inner_second",
        "reviewer",
        serde_json::json!({"done":false,"inner":"inner-1"}),
        serde_json::json!({"done":true,"inner":"inner-2"}),
    );
    steps.insert(
        StepId::from("matrix_body_side"),
        exact_json_step(
            "worker",
            serde_json::json!({"side":"body-side","done":true}),
        ),
    );

    let mut matrix_body_join = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"join":"body-join","inner":"{{steps.matrix_inner_second.inner}}","side":"{{steps.matrix_body_side.side}}","done":true}"#,
    );
    matrix_body_join.depends_on = vec![
        StepId::from("matrix_inner_second"),
        StepId::from("matrix_body_side"),
    ];
    steps.insert(StepId::from("matrix_body_join"), matrix_body_join);

    let mut matrix_fanin = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"matrix-audit","route":"{{steps.matrix_secondary.route}}","join":"{{steps.matrix_body_join.join}}","done":true}"#,
    );
    matrix_fanin.dispatch_mode = DispatchMode::FanIn;
    matrix_fanin.collection_policy = CollectionPolicy::All;
    matrix_fanin.depends_on = vec![
        StepId::from("matrix_secondary"),
        StepId::from("matrix_body_join"),
    ];
    steps.insert(StepId::from("matrix_fanin"), matrix_fanin);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"route":"{{steps.matrix_secondary.route}}","note":"{{steps.matrix_parallel_note.note}}","join":"{{steps.matrix_body_join.join}}","audit":"{{steps.matrix_fanin.0.output.kind}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("matrix_secondary"),
        StepId::from("matrix_parallel_note"),
        StepId::from("matrix_body_join"),
        StepId::from("matrix_fanin"),
    ];
    steps.insert(StepId::from("finalize_maximal_matrix"), finalize);

    let inner_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("matrix-inner-first-node"),
                frame_step("matrix_inner_first", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-inner-second-node"),
                frame_step("matrix_inner_second", vec!["matrix-inner-first-node"]),
            ),
        ]),
    };
    let outer_body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("matrix-inner-loop-node"),
                repeat_until_node(
                    "matrix_inner_loop",
                    vec![],
                    inner_body,
                    "steps.matrix_inner_second.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-body-side-node"),
                frame_step("matrix_body_side", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-body-join-node"),
                frame_step(
                    "matrix_body_join",
                    vec!["matrix-inner-loop-node", "matrix-body-side-node"],
                ),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("matrix-primary-node"),
                frame_step("matrix_primary", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-secondary-node"),
                frame_step("matrix_secondary", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-gather-node"),
                frame_step("matrix_gather", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-parallel-note-node"),
                frame_step("matrix_parallel_note", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-outer-loop-node"),
                repeat_until_node_any(
                    "matrix_outer_loop",
                    vec!["matrix-primary-node", "matrix-secondary-node"],
                    outer_body,
                    "steps.matrix_body_join.done",
                    2,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("matrix-fanin-node"),
                frame_step(
                    "matrix_fanin",
                    vec![
                        "matrix-secondary-node",
                        "matrix-gather-node",
                        "matrix-outer-loop-node",
                        "matrix-parallel-note-node",
                    ],
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-maximal-matrix-node"),
                frame_step("finalize_maximal_matrix", vec!["matrix-fanin-node"]),
            ),
        ]),
    };

    FlowSpec {
        description: Some(
            "branch + fanout + nested loop + fanin + parallel sibling maximal smoke".to_string(),
        ),
        steps,
        root: Some(root),
    }
}

fn build_persisted_branch_parallel_review_loop_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("persist_route_high"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"high","done":true}),
                condition_eq("params.route", serde_json::json!("high")),
            ),
            "persist_route_pick",
        ),
    );
    steps.insert(
        StepId::from("persist_route_low"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"low","done":true}),
                condition_eq("params.route", serde_json::json!("low")),
            ),
            "persist_route_pick",
        ),
    );
    steps.insert(
        StepId::from("persist_parallel_note"),
        exact_json_step(
            "lead",
            serde_json::json!({"note":"persist-note","done":true}),
        ),
    );
    steps.insert(
        StepId::from("persist_review"),
        prompt_prior_toggle_step(
            "reviewer",
            "loops.persisted_branch_review_loop.iterations.0.steps.persist_review.done",
            serde_json::json!({"done":false,"attempt":1,"stage":"review-1","approved":false}),
            serde_json::json!({"done":true,"attempt":2,"stage":"review-2","approved":true}),
        ),
    );
    let mut persist_echo = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"echo":"{{steps.persist_review.stage}}","route":"{{steps.persist_route_low.route}}","done":true}"#,
    );
    persist_echo.depends_on = vec![
        StepId::from("persist_review"),
        StepId::from("persist_route_low"),
    ];
    steps.insert(StepId::from("persist_review_echo"), persist_echo);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"route":"{{steps.persist_route_low.route}}","echo":"{{steps.persist_review_echo.echo}}","note":"{{steps.persist_parallel_note.note}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("persist_route_low"),
        StepId::from("persist_review_echo"),
        StepId::from("persist_parallel_note"),
    ];
    steps.insert(StepId::from("finalize_persisted_branch_review"), finalize);

    let body = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-review-node"),
                frame_step("persist_review", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-review-echo-node"),
                frame_step("persist_review_echo", vec!["persist-review-node"]),
            ),
        ]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-route-high-node"),
                frame_step("persist_route_high", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-route-low-node"),
                frame_step("persist_route_low", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-parallel-note-node"),
                frame_step("persist_parallel_note", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-branch-review-loop-node"),
                repeat_until_node_any(
                    "persisted_branch_review_loop",
                    vec!["persist-route-high-node", "persist-route-low-node"],
                    body,
                    "steps.persist_review.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-persisted-branch-review-node"),
                frame_step(
                    "finalize_persisted_branch_review",
                    vec![
                        "persist-branch-review-loop-node",
                        "persist-parallel-note-node",
                    ],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some(
            "branch + sibling note + persisted two-iteration review loop".to_string(),
        ),
        steps,
        root: Some(root),
    }
}

fn build_persisted_dual_loops_fanin_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("persist_left"),
        prompt_prior_toggle_step(
            "worker",
            "loops.persisted_left_loop.iterations.0.steps.persist_left.done",
            serde_json::json!({"done":false,"attempt":1,"left":"left-a"}),
            serde_json::json!({"done":true,"attempt":2,"left":"left-b"}),
        ),
    );
    steps.insert(
        StepId::from("persist_right"),
        prompt_prior_toggle_step(
            "reviewer",
            "loops.persisted_right_loop.iterations.0.steps.persist_right.done",
            serde_json::json!({"done":false,"attempt":1,"right":"right-a"}),
            serde_json::json!({"done":true,"attempt":2,"right":"right-b"}),
        ),
    );
    let mut audit = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"persisted-fanin","left":"{{steps.persist_left.left}}","right":"{{steps.persist_right.right}}","done":true}"#,
    );
    audit.dispatch_mode = DispatchMode::FanIn;
    audit.collection_policy = CollectionPolicy::All;
    audit.depends_on = vec![StepId::from("persist_left"), StepId::from("persist_right")];
    steps.insert(StepId::from("persist_fanin_audit"), audit);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"{{steps.persist_fanin_audit.0.output.kind}}","left":"{{steps.persist_fanin_audit.0.output.left}}","right":"{{steps.persist_fanin_audit.0.output.right}}","done":true}"#,
    );
    finalize.depends_on = vec![StepId::from("persist_fanin_audit")];
    steps.insert(StepId::from("finalize_persisted_dual_fanin"), finalize);

    let left_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-left-body-node"),
            frame_step("persist_left", vec![]),
        )]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-right-body-node"),
            frame_step("persist_right", vec![]),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-left-loop-node"),
                repeat_until_node(
                    "persisted_left_loop",
                    vec![],
                    left_body,
                    "steps.persist_left.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-right-loop-node"),
                repeat_until_node(
                    "persisted_right_loop",
                    vec![],
                    right_body,
                    "steps.persist_right.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-fanin-audit-node"),
                frame_step(
                    "persist_fanin_audit",
                    vec!["persist-left-loop-node", "persist-right-loop-node"],
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-persisted-dual-fanin-node"),
                frame_step(
                    "finalize_persisted_dual_fanin",
                    vec!["persist-fanin-audit-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("two persisted loops feeding a fan-in aggregate".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_persisted_sequential_loop_chain_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("persist_chain_left"),
        prompt_prior_toggle_step(
            "worker",
            "loops.persisted_chain_left_loop.iterations.0.steps.persist_chain_left.done",
            serde_json::json!({"done":false,"attempt":1,"left":"left-pass-1"}),
            serde_json::json!({"done":true,"attempt":2,"left":"left-pass-2"}),
        ),
    );
    steps.insert(
        StepId::from("persist_chain_bridge"),
        exact_json_step(
            "lead",
            serde_json::json!({"bridge":"bridge-pass","done":true}),
        ),
    );
    steps.insert(
        StepId::from("persist_chain_right"),
        prompt_prior_toggle_step(
            "reviewer",
            "loops.persisted_chain_right_loop.iterations.0.steps.persist_chain_right.done",
            serde_json::json!({"done":false,"attempt":1,"right":"right-pass-1"}),
            serde_json::json!({"done":true,"attempt":2,"right":"right-pass-2"}),
        ),
    );
    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"left":"{{steps.persist_chain_left.left}}","bridge":"{{steps.persist_chain_bridge.bridge}}","right":"{{steps.persist_chain_right.right}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("persist_chain_left"),
        StepId::from("persist_chain_bridge"),
        StepId::from("persist_chain_right"),
    ];
    steps.insert(StepId::from("finalize_persisted_chain"), finalize);

    let left_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-chain-left-body-node"),
            frame_step("persist_chain_left", vec![]),
        )]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-chain-right-body-node"),
            frame_step("persist_chain_right", vec![]),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-chain-left-loop-node"),
                repeat_until_node(
                    "persisted_chain_left_loop",
                    vec![],
                    left_body,
                    "steps.persist_chain_left.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-chain-bridge-node"),
                frame_step("persist_chain_bridge", vec!["persist-chain-left-loop-node"]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-chain-right-loop-node"),
                repeat_until_node(
                    "persisted_chain_right_loop",
                    vec!["persist-chain-bridge-node"],
                    right_body,
                    "steps.persist_chain_right.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-persisted-chain-node"),
                frame_step(
                    "finalize_persisted_chain",
                    vec!["persist-chain-right-loop-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("sequential chain of two persisted loops".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_persisted_three_sibling_loops_join_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("persist_alpha"),
        prompt_prior_toggle_step(
            "worker",
            "loops.persisted_alpha_loop.iterations.0.steps.persist_alpha.done",
            serde_json::json!({"done":false,"attempt":1,"alpha":"alpha-1"}),
            serde_json::json!({"done":true,"attempt":2,"alpha":"alpha-2"}),
        ),
    );
    steps.insert(
        StepId::from("persist_beta"),
        prompt_prior_toggle_step(
            "reviewer",
            "loops.persisted_beta_loop.iterations.0.steps.persist_beta.done",
            serde_json::json!({"done":false,"attempt":1,"beta":"beta-1"}),
            serde_json::json!({"done":true,"attempt":2,"beta":"beta-2"}),
        ),
    );
    steps.insert(
        StepId::from("persist_gamma"),
        prompt_prior_toggle_step(
            "lead",
            "loops.persisted_gamma_loop.iterations.0.steps.persist_gamma.done",
            serde_json::json!({"done":false,"attempt":1,"gamma":"gamma-1"}),
            serde_json::json!({"done":true,"attempt":2,"gamma":"gamma-2"}),
        ),
    );
    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"alpha":"{{steps.persist_alpha.alpha}}","beta":"{{steps.persist_beta.beta}}","gamma":"{{steps.persist_gamma.gamma}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("persist_alpha"),
        StepId::from("persist_beta"),
        StepId::from("persist_gamma"),
    ];
    steps.insert(StepId::from("finalize_persisted_three_loops"), finalize);

    let alpha_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-alpha-body-node"),
            frame_step("persist_alpha", vec![]),
        )]),
    };
    let beta_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-beta-body-node"),
            frame_step("persist_beta", vec![]),
        )]),
    };
    let gamma_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-gamma-body-node"),
            frame_step("persist_gamma", vec![]),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-alpha-loop-node"),
                repeat_until_node(
                    "persisted_alpha_loop",
                    vec![],
                    alpha_body,
                    "steps.persist_alpha.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-beta-loop-node"),
                repeat_until_node(
                    "persisted_beta_loop",
                    vec![],
                    beta_body,
                    "steps.persist_beta.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-gamma-loop-node"),
                repeat_until_node(
                    "persisted_gamma_loop",
                    vec![],
                    gamma_body,
                    "steps.persist_gamma.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-persisted-three-loops-node"),
                frame_step(
                    "finalize_persisted_three_loops",
                    vec![
                        "persist-alpha-loop-node",
                        "persist-beta-loop-node",
                        "persist-gamma-loop-node",
                    ],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some("three sibling persisted loops with downstream join".to_string()),
        steps,
        root: Some(root),
    }
}

fn build_persisted_branch_dual_loops_audit_flow() -> FlowSpec {
    let mut steps = IndexMap::new();
    steps.insert(
        StepId::from("persist_audit_route_primary"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"primary","done":true}),
                condition_eq("params.route", serde_json::json!("primary")),
            ),
            "persist_audit_route",
        ),
    );
    steps.insert(
        StepId::from("persist_audit_route_secondary"),
        with_branch(
            exact_json_step_with_condition(
                "worker",
                serde_json::json!({"route":"secondary","done":true}),
                condition_eq("params.route", serde_json::json!("secondary")),
            ),
            "persist_audit_route",
        ),
    );
    steps.insert(
        StepId::from("persist_audit_left"),
        prompt_prior_toggle_step(
            "worker",
            "loops.persisted_audit_left_loop.iterations.0.steps.persist_audit_left.done",
            serde_json::json!({"done":false,"attempt":1,"left":"audit-left-1"}),
            serde_json::json!({"done":true,"attempt":2,"left":"audit-left-2"}),
        ),
    );
    steps.insert(
        StepId::from("persist_audit_right"),
        prompt_prior_toggle_step(
            "reviewer",
            "loops.persisted_audit_right_loop.iterations.0.steps.persist_audit_right.done",
            serde_json::json!({"done":false,"attempt":1,"right":"audit-right-1"}),
            serde_json::json!({"done":true,"attempt":2,"right":"audit-right-2"}),
        ),
    );
    let mut audit = json_step(
        "analyst",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"kind":"persist-audit","route":"{{steps.persist_audit_route_secondary.route}}","left":"{{steps.persist_audit_left.left}}","right":"{{steps.persist_audit_right.right}}","done":true}"#,
    );
    audit.dispatch_mode = DispatchMode::FanOut;
    audit.collection_policy = CollectionPolicy::All;
    audit.depends_on = vec![
        StepId::from("persist_audit_route_secondary"),
        StepId::from("persist_audit_left"),
        StepId::from("persist_audit_right"),
    ];
    steps.insert(StepId::from("persist_branch_loop_audit"), audit);

    let mut finalize = json_step(
        "lead",
        r#"You are a deterministic flow smoke-test responder.
Return raw JSON only. No markdown.
Return exactly this JSON object and nothing else:
{"route":"{{steps.persist_audit_route_secondary.route}}","left":"{{steps.persist_audit_left.left}}","right":"{{steps.persist_audit_right.right}}","done":true}"#,
    );
    finalize.depends_on = vec![
        StepId::from("persist_audit_route_secondary"),
        StepId::from("persist_branch_loop_audit"),
    ];
    steps.insert(StepId::from("finalize_persist_branch_dual_audit"), finalize);

    let left_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-audit-left-body-node"),
            frame_step("persist_audit_left", vec![]),
        )]),
    };
    let right_body = FrameSpec {
        nodes: IndexMap::from([(
            meerkat_mob::FlowNodeId::from("persist-audit-right-body-node"),
            frame_step("persist_audit_right", vec![]),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                meerkat_mob::FlowNodeId::from("persist-audit-route-primary-node"),
                frame_step("persist_audit_route_primary", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-audit-route-secondary-node"),
                frame_step("persist_audit_route_secondary", vec![]),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-audit-left-loop-node"),
                repeat_until_node_any(
                    "persisted_audit_left_loop",
                    vec![
                        "persist-audit-route-primary-node",
                        "persist-audit-route-secondary-node",
                    ],
                    left_body,
                    "steps.persist_audit_left.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-audit-right-loop-node"),
                repeat_until_node_any(
                    "persisted_audit_right_loop",
                    vec![
                        "persist-audit-route-primary-node",
                        "persist-audit-route-secondary-node",
                    ],
                    right_body,
                    "steps.persist_audit_right.done",
                    3,
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("persist-branch-loop-audit-node"),
                frame_step(
                    "persist_branch_loop_audit",
                    vec![
                        "persist-audit-left-loop-node",
                        "persist-audit-right-loop-node",
                    ],
                ),
            ),
            (
                meerkat_mob::FlowNodeId::from("finalize-persist-branch-dual-audit-node"),
                frame_step(
                    "finalize_persist_branch_dual_audit",
                    vec!["persist-branch-loop-audit-node"],
                ),
            ),
        ]),
    };

    FlowSpec {
        description: Some(
            "branch-gated sibling persisted loops feeding an analyst fanout".to_string(),
        ),
        steps,
        root: Some(root),
    }
}

async fn setup_flow_mob(
    models: &FlowSmokeModels,
) -> (MobHandle, Arc<dyn MobSessionService>, TempDir) {
    let temp = TempDir::new().expect("temp dir");
    let paths = FlowSmokePaths::new(temp.path());

    let session_service = persistent_service(&paths);
    let mob_service: Arc<dyn MobSessionService> = session_service.clone();
    let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let storage =
        MobStorage::persistent(&paths.mob_db_path).expect("create persistent mob storage");

    let handle = MobBuilder::new(flow_definition(models), storage)
        .with_session_service(mob_service.clone())
        .with_runtime_adapter(runtime_adapter)
        .create()
        .await
        .expect("create flow smoke mob");

    handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from("worker-1"),
        ))
        .await
        .expect("spawn worker");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "reviewer",
            AgentIdentity::from("reviewer-1"),
        ))
        .await
        .expect("spawn reviewer");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "analyst",
            AgentIdentity::from("analyst-1"),
        ))
        .await
        .expect("spawn analyst-1");
    handle
        .spawn_spec(SpawnMemberSpec::new(
            "analyst",
            AgentIdentity::from("analyst-2"),
        ))
        .await
        .expect("spawn analyst-2");

    (handle, mob_service, temp)
}

async fn wait_for_run_terminal(handle: &MobHandle, run_id: &meerkat_mob::RunId) -> MobRun {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let run = handle
            .flow_status(run_id.clone())
            .await
            .expect("flow_status should succeed")
            .expect("run should exist");
        if run.status.is_terminal() {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for flow run {run_id} to finish; last run snapshot: {run:?}"
        );
        sleep(Duration::from_millis(500)).await;
    }
}

fn completed_root_output<'a>(run: &'a MobRun, step_id: &str) -> &'a Value {
    run.root_step_outputs
        .get(&StepId::from(step_id))
        .unwrap_or_else(|| panic!("missing root_step_outputs entry for {step_id}: {run:?}"))
}

fn loop_iterations<'a>(run: &'a MobRun, loop_id: &str) -> &'a Vec<IndexMap<StepId, Value>> {
    run.loop_iteration_outputs
        .get(&LoopId::from(loop_id))
        .unwrap_or_else(|| panic!("missing loop outputs for {loop_id}: {run:?}"))
}

fn count_loop_iterations(run: &MobRun, loop_id: &str) -> usize {
    let loop_instance_ids: BTreeSet<_> = run
        .loops
        .iter()
        .filter(|(_, snapshot)| snapshot.kernel_state.loop_id == loop_id)
        .map(|(instance_id, _)| instance_id.clone())
        .collect();
    assert!(
        !loop_instance_ids.is_empty(),
        "missing loop snapshot for logical loop_id={loop_id}: {run:?}"
    );
    run.loop_iteration_ledger
        .iter()
        .filter(|entry| loop_instance_ids.contains(&entry.loop_instance_id))
        .count()
}

fn completed_count(run: &MobRun, step_id: &str) -> usize {
    run.step_ledger
        .iter()
        .filter(|entry| {
            entry.step_id == step_id && entry.status == meerkat_mob::StepRunStatus::Completed
        })
        .count()
}

fn skipped_count(run: &MobRun, step_id: &str) -> usize {
    run.step_ledger
        .iter()
        .filter(|entry| {
            entry.step_id == step_id && entry.status == meerkat_mob::StepRunStatus::Skipped
        })
        .count()
}

fn assert_completed_without_failures(run: &MobRun, label: &str) {
    assert_eq!(
        run.status,
        MobRunStatus::Completed,
        "{label} should complete: {run:?}"
    );
    assert!(
        run.failure_ledger.is_empty(),
        "{label} should not record failures: {run:?}"
    );
}

async fn run_smoke_flow_or_skip(test_name: &str, flow_id: &str, params: Value) -> Option<MobRun> {
    let Some(models) = flow_smoke_models() else {
        eprintln!(
            "Skipping {test_name}: no matching API key for FLOW_SMOKE_MODEL/SMOKE_MODEL or fast default models"
        );
        return None;
    };

    let _permit = acquire_flow_runtime_smoke_permit(test_name).await;
    eprintln!(
        "Running {test_name} with {}",
        flow_smoke_models_label(&models)
    );
    let (handle, _service, _temp) = setup_flow_mob(&models).await;
    let run_id = handle
        .run_flow(FlowId::from(flow_id), params)
        .await
        .unwrap_or_else(|error| panic!("run {flow_id} flow for {test_name}: {error}"));
    Some(wait_for_run_terminal(&handle, &run_id).await)
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_fanout_parallel_review_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_fanout_parallel_review_loop_smoke",
        "fanout_review_loop",
        serde_json::json!({ "topic": "meerkat runtime dogma" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "fanout review loop flow");

    let gather = completed_root_output(&run, "gather");
    let gather_map = gather
        .as_object()
        .expect("fanout gather output should be an object");
    assert_eq!(
        gather_map.len(),
        2,
        "fanout gather output should contain both analyst targets: {gather:?}"
    );
    for value in gather_map.values() {
        assert_eq!(
            value.get("topic"),
            Some(&serde_json::json!("meerkat runtime dogma")),
            "fanout gather outputs should preserve the topic: {gather:?}"
        );
        assert_eq!(
            value.get("done"),
            Some(&serde_json::json!(true)),
            "fanout gather outputs should carry done=true: {gather:?}"
        );
    }

    let review_iterations = loop_iterations(&run, "review_loop");
    assert_eq!(
        review_iterations.len(),
        2,
        "review loop should take exactly two iterations: {review_iterations:?}"
    );
    assert_eq!(
        review_iterations[0].get(&StepId::from("review")),
        Some(&serde_json::json!({
            "approved": false,
            "done": false,
            "summary": "needs one revision"
        })),
        "first review iteration should request a revision"
    );
    assert_eq!(
        review_iterations[1].get(&StepId::from("review")),
        Some(&serde_json::json!({
            "approved": true,
            "done": true,
            "summary": "approved on second pass"
        })),
        "second review iteration should approve the draft"
    );
    assert_eq!(
        count_loop_iterations(&run, "review_loop"),
        2,
        "loop iteration ledger should record exactly two review iterations"
    );

    let finalize = completed_root_output(&run, "finalize");
    assert_eq!(
        finalize.get("done"),
        Some(&serde_json::json!(true)),
        "finalize output should mark done=true: {finalize:?}"
    );
    let final_message = finalize
        .get("final_message")
        .and_then(Value::as_str)
        .expect("finalize.final_message should be a string");
    assert!(
        final_message.contains("approved on second pass")
            && final_message.contains("parallel sibling complete"),
        "finalize output should reflect downstream access to loop + sibling state: {finalize:?}"
    );

    assert!(
        completed_count(&run, "draft") >= 2 && completed_count(&run, "review") >= 2,
        "loop body steps should complete once per iteration: {run:?}"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_dual_sibling_loops_join_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_dual_sibling_loops_join_smoke",
        "dual_loops_join",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "dual sibling loops flow");

    let left_iterations = loop_iterations(&run, "left_loop");
    let right_iterations = loop_iterations(&run, "right_loop");
    assert_eq!(
        left_iterations.len(),
        2,
        "left loop should take exactly two iterations: {left_iterations:?}"
    );
    assert_eq!(
        right_iterations.len(),
        2,
        "right loop should take exactly two iterations: {right_iterations:?}"
    );
    assert_eq!(
        left_iterations[1].get(&StepId::from("left")),
        Some(&serde_json::json!({
            "done": true,
            "attempt": 2,
            "path": "left-second"
        })),
        "left loop should converge on the second iteration"
    );
    assert_eq!(
        right_iterations[1].get(&StepId::from("right")),
        Some(&serde_json::json!({
            "done": true,
            "attempt": 2,
            "path": "right-second"
        })),
        "right loop should converge on the second iteration"
    );
    assert_eq!(
        count_loop_iterations(&run, "left_loop"),
        2,
        "left loop ledger should record two iterations"
    );
    assert_eq!(
        count_loop_iterations(&run, "right_loop"),
        2,
        "right loop ledger should record two iterations"
    );

    let join = completed_root_output(&run, "join");
    assert_eq!(
        join.get("done"),
        Some(&serde_json::json!(true)),
        "join output should mark done=true: {join:?}"
    );
    assert_eq!(
        join.get("final_message"),
        Some(&serde_json::json!("joined left-second and right-second")),
        "join should see the last iteration output from both sibling loops"
    );
    assert!(
        completed_count(&run, "left") >= 2
            && completed_count(&run, "right") >= 2
            && completed_count(&run, "join") >= 1,
        "step ledger should show both loop bodies and the downstream join completing: {run:?}"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_branch_then_review_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_branch_then_review_loop_smoke",
        "branch_then_review_loop",
        serde_json::json!({ "severity": "minor" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "branch then review loop flow");
    assert_eq!(loop_iterations(&run, "branch_review_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "branch_review_loop"), 1);
    assert!(completed_count(&run, "minor_route") >= 1);
    assert!(skipped_count(&run, "critical_route") >= 1);

    let finalize = completed_root_output(&run, "finalize_branch_loop");
    assert_eq!(finalize.get("selected"), Some(&serde_json::json!("minor")));
    assert_eq!(
        finalize.get("revision"),
        Some(&serde_json::json!("revise-second"))
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_parallel_body_siblings_join_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_parallel_body_siblings_join_smoke",
        "parallel_body_siblings_join",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "parallel body siblings flow");
    let iterations = loop_iterations(&run, "parallel_body_loop");
    assert_eq!(iterations.len(), 1);
    assert_eq!(count_loop_iterations(&run, "parallel_body_loop"), 1);
    assert_eq!(
        iterations[0].get(&StepId::from("body_join")),
        Some(&serde_json::json!({
            "done": true,
            "tag": "join-final",
            "left": "L2",
            "right": "R2"
        }))
    );

    let finalize = completed_root_output(&run, "finalize_parallel_body");
    assert_eq!(finalize.get("left"), Some(&serde_json::json!("L2")));
    assert_eq!(finalize.get("right"), Some(&serde_json::json!("R2")));
    assert_eq!(finalize.get("join"), Some(&serde_json::json!("join-final")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_nested_outer_inner_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_nested_outer_inner_loop_smoke",
        "nested_outer_inner_loop",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "nested outer inner loop flow");
    assert_eq!(loop_iterations(&run, "outer_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "outer_loop"), 1);
    assert_eq!(loop_iterations(&run, "inner_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "inner_loop"), 1);
    assert!(completed_count(&run, "inner_second") >= 1);
    assert!(completed_count(&run, "outer_second") >= 1);

    let finalize = completed_root_output(&run, "finalize_nested_loop");
    assert_eq!(finalize.get("outer"), Some(&serde_json::json!("outer-2")));
    assert_eq!(finalize.get("inner"), Some(&serde_json::json!("inner-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_fanin_after_parallel_loops_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_fanin_after_parallel_loops_smoke",
        "fanin_after_parallel_loops",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "fanin after parallel loops flow");
    assert_eq!(loop_iterations(&run, "fanin_left_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "fanin_left_loop"), 1);
    assert_eq!(loop_iterations(&run, "fanin_right_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "fanin_right_loop"), 1);

    let audit = completed_root_output(&run, "fanin_audit");
    let audit_array = audit
        .as_array()
        .expect("fanin_audit output should be an array");
    assert_eq!(
        audit_array.len(),
        2,
        "fanin should produce two analyst outputs"
    );

    let finalize = completed_root_output(&run, "finalize_fanin");
    assert_eq!(finalize.get("kind"), Some(&serde_json::json!("audit")));
    assert_eq!(finalize.get("left"), Some(&serde_json::json!("left-2")));
    assert_eq!(finalize.get("right"), Some(&serde_json::json!("right-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_conditional_skip_inside_body_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_conditional_skip_inside_body_loop_smoke",
        "conditional_skip_inside_body_loop",
        serde_json::json!({ "mode": "fallback" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "conditional skip inside body loop flow");
    assert_eq!(loop_iterations(&run, "conditional_body_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "conditional_body_loop"), 1);
    assert!(skipped_count(&run, "optional_path") >= 1);
    assert!(completed_count(&run, "fallback_path") >= 1);

    let finalize = completed_root_output(&run, "finalize_conditional_body");
    assert_eq!(finalize.get("path"), Some(&serde_json::json!("fallback")));
    assert_eq!(finalize.get("phase"), Some(&serde_json::json!("phase-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_three_way_branch_loop_audit_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_three_way_branch_loop_audit_smoke",
        "three_way_branch_loop_audit",
        serde_json::json!({ "route": "gamma" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "three-way branch loop flow");
    assert_eq!(loop_iterations(&run, "branch_audit_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "branch_audit_loop"), 1);
    assert!(completed_count(&run, "gamma_route") >= 1);
    assert!(skipped_count(&run, "alpha_route") >= 1);
    assert!(skipped_count(&run, "beta_route") >= 1);

    let branch_audit = completed_root_output(&run, "branch_audit");
    let branch_audit_map = branch_audit
        .as_object()
        .expect("branch_audit output should be a fanout object");
    assert_eq!(branch_audit_map.len(), 2);

    let finalize = completed_root_output(&run, "finalize_three_way_branch");
    assert_eq!(finalize.get("route"), Some(&serde_json::json!("gamma")));
    assert_eq!(finalize.get("phase"), Some(&serde_json::json!("audit-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_sequential_loop_chain_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_sequential_loop_chain_smoke",
        "sequential_loop_chain",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "sequential loop chain flow");
    assert_eq!(loop_iterations(&run, "chain_left_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "chain_left_loop"), 1);
    assert_eq!(loop_iterations(&run, "chain_right_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "chain_right_loop"), 1);
    assert!(completed_count(&run, "chain_bridge") >= 1);

    let finalize = completed_root_output(&run, "finalize_sequential_chain");
    assert_eq!(finalize.get("left"), Some(&serde_json::json!("left-2")));
    assert_eq!(finalize.get("bridge"), Some(&serde_json::json!("handoff")));
    assert_eq!(finalize.get("right"), Some(&serde_json::json!("right-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_three_sibling_loops_join_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_three_sibling_loops_join_smoke",
        "three_sibling_loops_join",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "three sibling loops flow");
    assert_eq!(loop_iterations(&run, "alpha_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "alpha_loop"), 1);
    assert_eq!(loop_iterations(&run, "beta_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "beta_loop"), 1);
    assert_eq!(loop_iterations(&run, "gamma_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "gamma_loop"), 1);

    let finalize = completed_root_output(&run, "finalize_three_sibling_loops");
    assert_eq!(finalize.get("alpha"), Some(&serde_json::json!("A2")));
    assert_eq!(finalize.get("beta"), Some(&serde_json::json!("B2")));
    assert_eq!(finalize.get("gamma"), Some(&serde_json::json!("G2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_outer_branch_inner_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_outer_branch_inner_loop_smoke",
        "outer_branch_inner_loop",
        serde_json::json!({ "route": "blue" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "outer branch inner loop flow");
    assert_eq!(loop_iterations(&run, "outer_branch_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "outer_branch_loop"), 1);
    assert_eq!(loop_iterations(&run, "branch_inner_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "branch_inner_loop"), 1);
    assert!(completed_count(&run, "route_blue") >= 1);
    assert!(skipped_count(&run, "route_red") >= 1);

    let finalize = completed_root_output(&run, "finalize_outer_branch_inner");
    assert_eq!(finalize.get("route"), Some(&serde_json::json!("blue")));
    assert_eq!(
        finalize.get("outer"),
        Some(&serde_json::json!("outer-branch"))
    );
    assert_eq!(finalize.get("inner"), Some(&serde_json::json!("inner-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_maximal_matrix_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_maximal_matrix_smoke",
        "maximal_matrix",
        serde_json::json!({ "route": "secondary" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "maximal matrix flow");
    assert_eq!(loop_iterations(&run, "matrix_outer_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "matrix_outer_loop"), 1);
    assert_eq!(loop_iterations(&run, "matrix_inner_loop").len(), 1);
    assert_eq!(count_loop_iterations(&run, "matrix_inner_loop"), 1);
    assert!(skipped_count(&run, "matrix_primary") >= 1);
    assert!(completed_count(&run, "matrix_parallel_note") >= 1);

    let gather = completed_root_output(&run, "matrix_gather");
    let gather_map = gather
        .as_object()
        .expect("matrix_gather output should be an object");
    assert_eq!(gather_map.len(), 2);

    let fanin = completed_root_output(&run, "matrix_fanin");
    let fanin_array = fanin
        .as_array()
        .expect("matrix_fanin output should be an array");
    assert_eq!(fanin_array.len(), 2);

    let finalize = completed_root_output(&run, "finalize_maximal_matrix");
    assert_eq!(finalize.get("route"), Some(&serde_json::json!("secondary")));
    assert_eq!(
        finalize.get("note"),
        Some(&serde_json::json!("parallel-note"))
    );
    assert_eq!(finalize.get("join"), Some(&serde_json::json!("body-join")));
    assert_eq!(
        finalize.get("audit"),
        Some(&serde_json::json!("matrix-audit"))
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_persisted_branch_parallel_review_loop_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_persisted_branch_parallel_review_loop_smoke",
        "persisted_branch_parallel_review_loop",
        serde_json::json!({ "route": "low" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "persisted branch parallel review loop");
    assert_eq!(
        loop_iterations(&run, "persisted_branch_review_loop").len(),
        2
    );
    assert_eq!(
        count_loop_iterations(&run, "persisted_branch_review_loop"),
        2
    );
    assert!(completed_count(&run, "persist_route_low") >= 1);
    assert!(skipped_count(&run, "persist_route_high") >= 1);
    assert!(completed_count(&run, "persist_review") >= 2);
    assert!(completed_count(&run, "persist_review_echo") >= 2);

    let finalize = completed_root_output(&run, "finalize_persisted_branch_review");
    assert_eq!(finalize.get("route"), Some(&serde_json::json!("low")));
    assert_eq!(finalize.get("echo"), Some(&serde_json::json!("review-2")));
    assert_eq!(
        finalize.get("note"),
        Some(&serde_json::json!("persist-note"))
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_persisted_dual_loops_fanin_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_persisted_dual_loops_fanin_smoke",
        "persisted_dual_loops_fanin",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "persisted dual loops fanin");
    assert_eq!(loop_iterations(&run, "persisted_left_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_left_loop"), 2);
    assert_eq!(loop_iterations(&run, "persisted_right_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_right_loop"), 2);

    let audit = completed_root_output(&run, "persist_fanin_audit");
    let audit_array = audit
        .as_array()
        .expect("persist_fanin_audit output should be an array");
    assert_eq!(audit_array.len(), 2);

    let finalize = completed_root_output(&run, "finalize_persisted_dual_fanin");
    assert_eq!(
        finalize.get("kind"),
        Some(&serde_json::json!("persisted-fanin"))
    );
    assert_eq!(finalize.get("left"), Some(&serde_json::json!("left-b")));
    assert_eq!(finalize.get("right"), Some(&serde_json::json!("right-b")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_persisted_sequential_loop_chain_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_persisted_sequential_loop_chain_smoke",
        "persisted_sequential_loop_chain",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "persisted sequential loop chain");
    assert_eq!(loop_iterations(&run, "persisted_chain_left_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_chain_left_loop"), 2);
    assert_eq!(loop_iterations(&run, "persisted_chain_right_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_chain_right_loop"), 2);
    assert!(completed_count(&run, "persist_chain_bridge") >= 1);

    let finalize = completed_root_output(&run, "finalize_persisted_chain");
    assert_eq!(
        finalize.get("left"),
        Some(&serde_json::json!("left-pass-2"))
    );
    assert_eq!(
        finalize.get("bridge"),
        Some(&serde_json::json!("bridge-pass"))
    );
    assert_eq!(
        finalize.get("right"),
        Some(&serde_json::json!("right-pass-2"))
    );
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_persisted_three_sibling_loops_join_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_persisted_three_sibling_loops_join_smoke",
        "persisted_three_sibling_loops_join",
        serde_json::json!({}),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "persisted three sibling loops");
    assert_eq!(loop_iterations(&run, "persisted_alpha_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_alpha_loop"), 2);
    assert_eq!(loop_iterations(&run, "persisted_beta_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_beta_loop"), 2);
    assert_eq!(loop_iterations(&run, "persisted_gamma_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_gamma_loop"), 2);

    let finalize = completed_root_output(&run, "finalize_persisted_three_loops");
    assert_eq!(finalize.get("alpha"), Some(&serde_json::json!("alpha-2")));
    assert_eq!(finalize.get("beta"), Some(&serde_json::json!("beta-2")));
    assert_eq!(finalize.get("gamma"), Some(&serde_json::json!("gamma-2")));
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_flow_runtime_persisted_branch_dual_loops_audit_smoke() {
    let Some(run) = run_smoke_flow_or_skip(
        "e2e_flow_runtime_persisted_branch_dual_loops_audit_smoke",
        "persisted_branch_dual_loops_audit",
        serde_json::json!({ "route": "secondary" }),
    )
    .await
    else {
        return;
    };

    assert_completed_without_failures(&run, "persisted branch dual loops audit");
    assert_eq!(loop_iterations(&run, "persisted_audit_left_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_audit_left_loop"), 2);
    assert_eq!(loop_iterations(&run, "persisted_audit_right_loop").len(), 2);
    assert_eq!(count_loop_iterations(&run, "persisted_audit_right_loop"), 2);
    assert!(completed_count(&run, "persist_audit_route_secondary") >= 1);
    assert!(skipped_count(&run, "persist_audit_route_primary") >= 1);

    let audit = completed_root_output(&run, "persist_branch_loop_audit");
    let audit_map = audit
        .as_object()
        .expect("persist_branch_loop_audit output should be a fanout object");
    assert_eq!(audit_map.len(), 2);

    let finalize = completed_root_output(&run, "finalize_persist_branch_dual_audit");
    assert_eq!(finalize.get("route"), Some(&serde_json::json!("secondary")));
    assert_eq!(
        finalize.get("left"),
        Some(&serde_json::json!("audit-left-2"))
    );
    assert_eq!(
        finalize.get("right"),
        Some(&serde_json::json!("audit-right-2"))
    );
}
