use indexmap::IndexMap;
use meerkat_contracts::wire::{
    MobBackendConfigInput, MobCollectionPolicyInput, MobConditionExprInput, MobDefinitionInput,
    MobDependencyModeInput, MobDispatchModeInput, MobEventRouterConfigInput, MobFlowNodeInput,
    MobFlowSpecInput, MobFlowStepInput, MobFrameSpecInput, MobFrameStepInput, MobLimitsSpecInput,
    MobPolicyModeInput, MobProfileInput, MobRepeatUntilInput, MobSkillSourceInput,
    MobSpawnPolicyInput, MobStepOutputFormatInput, MobTopologySpecInput, WireMobBackendKind,
    WireMobRuntimeMode,
};
use meerkat_core::types::ContentInput;
use meerkat_mob::{
    BranchId, FlowId, FlowNodeId, LoopId, MobBackendKind, MobDefinition, MobId, MobRuntimeMode,
    Profile, ProfileName, StepId, ToolConfig,
    definition::{
        BackendConfig, CollectionPolicy, ConditionExpr, DependencyMode, DispatchMode,
        EventRouterConfig, ExternalBackendConfig, FlowNodeSpec, FlowSpec, FlowStepSpec, FrameSpec,
        FrameStepSpec, LimitsSpec, McpServerConfig, OrchestratorConfig, PolicyMode,
        RepeatUntilSpec, RoleWiringRule, SessionCleanupPolicy, SkillSource, SpawnPolicyConfig,
        StepOutputFormat, SupervisorSpec, TopologyRule, TopologySpec, WiringRules,
    },
};
use std::convert::TryFrom;

/// Decode the canonical public mob-create definition into the internal runtime definition.
///
/// The public wire contract owns the accepted shape; this conversion is an
/// explicit mechanical mapping into `meerkat_mob::MobDefinition`.
pub fn decode_public_mob_definition(input: MobDefinitionInput) -> Result<MobDefinition, String> {
    Ok(MobDefinition {
        id: MobId::from(input.id),
        orchestrator: input.orchestrator.map(|orchestrator| OrchestratorConfig {
            profile: ProfileName::from(orchestrator.profile),
        }),
        profiles: input
            .profiles
            .into_iter()
            .map(|(profile_name, profile)| decode_profile(profile_name, profile))
            .collect::<Result<_, _>>()?,
        mcp_servers: input
            .mcp_servers
            .into_iter()
            .map(|(name, config)| {
                Ok((
                    name,
                    McpServerConfig {
                        command: config.command,
                        url: config.url,
                        env: config.env,
                    },
                ))
            })
            .collect::<Result<_, String>>()?,
        wiring: WiringRules {
            auto_wire_orchestrator: input.wiring.auto_wire_orchestrator,
            role_wiring: input
                .wiring
                .role_wiring
                .into_iter()
                .map(|rule| RoleWiringRule {
                    a: ProfileName::from(rule.a),
                    b: ProfileName::from(rule.b),
                })
                .collect(),
        },
        skills: input
            .skills
            .into_iter()
            .map(|(name, source)| Ok((name, decode_skill_source(source))))
            .collect::<Result<_, String>>()?,
        backend: decode_backend(input.backend),
        flows: input
            .flows
            .into_iter()
            .map(|(flow_id, flow)| decode_flow_spec(flow_id, flow))
            .collect::<Result<_, _>>()?,
        topology: input.topology.map(decode_topology),
        supervisor: input.supervisor.map(|supervisor| SupervisorSpec {
            role: ProfileName::from(supervisor.role),
            escalation_threshold: supervisor.escalation_threshold,
        }),
        limits: input.limits.map(decode_limits),
        spawn_policy: input.spawn_policy.map(decode_spawn_policy),
        event_router: input.event_router.map(decode_event_router),
        owner_session_id: None,
        session_cleanup_policy: SessionCleanupPolicy::Manual,
        is_implicit: false,
    })
}

fn decode_profile(
    profile_name: String,
    input: MobProfileInput,
) -> Result<(ProfileName, Profile), String> {
    Ok((
        ProfileName::from(profile_name),
        Profile {
            model: input.model,
            skills: input.skills,
            tools: ToolConfig {
                builtins: input.tools.builtins,
                shell: input.tools.shell,
                comms: input.tools.comms,
                memory: input.tools.memory,
                mob: input.tools.mob,
                mob_tasks: input.tools.mob_tasks,
                mcp: input.tools.mcp,
                rust_bundles: Vec::new(),
            },
            peer_description: input.peer_description,
            external_addressable: input.external_addressable,
            backend: input.backend.map(decode_backend_kind),
            runtime_mode: decode_runtime_mode(input.runtime_mode),
            max_inline_peer_notifications: input.max_inline_peer_notifications,
            output_schema: input.output_schema,
            provider_params: input.provider_params,
        },
    ))
}

fn decode_skill_source(input: MobSkillSourceInput) -> SkillSource {
    match input {
        MobSkillSourceInput::Inline { content } => SkillSource::Inline { content },
        MobSkillSourceInput::Path { path } => SkillSource::Path { path },
    }
}

fn decode_backend(input: MobBackendConfigInput) -> BackendConfig {
    BackendConfig {
        default: decode_backend_kind(input.default),
        external: input.external.map(|external| ExternalBackendConfig {
            address_base: external.address_base,
        }),
    }
}

fn decode_backend_kind(input: WireMobBackendKind) -> MobBackendKind {
    match input {
        WireMobBackendKind::Session => MobBackendKind::Session,
        WireMobBackendKind::External => MobBackendKind::External,
    }
}

fn decode_runtime_mode(input: WireMobRuntimeMode) -> MobRuntimeMode {
    match input {
        WireMobRuntimeMode::AutonomousHost => MobRuntimeMode::AutonomousHost,
        WireMobRuntimeMode::TurnDriven => MobRuntimeMode::TurnDriven,
    }
}

fn decode_flow_spec(
    flow_id: String,
    input: MobFlowSpecInput,
) -> Result<(FlowId, FlowSpec), String> {
    Ok((
        FlowId::from(flow_id),
        FlowSpec {
            description: input.description,
            steps: input
                .steps
                .into_iter()
                .map(|(step_id, step)| decode_flow_step(step_id, step))
                .collect::<Result<IndexMap<_, _>, _>>()?,
            root: input.root.map(decode_frame_spec).transpose()?,
        },
    ))
}

fn decode_flow_step(
    step_id: String,
    input: MobFlowStepInput,
) -> Result<(StepId, FlowStepSpec), String> {
    Ok((
        StepId::from(step_id),
        FlowStepSpec {
            role: ProfileName::from(input.role),
            message: ContentInput::try_from(input.message)
                .map_err(|error| format!("invalid flow step message: {error}"))?,
            depends_on: input.depends_on.into_iter().map(StepId::from).collect(),
            dispatch_mode: decode_dispatch_mode(input.dispatch_mode),
            collection_policy: decode_collection_policy(input.collection_policy),
            condition: input.condition.map(decode_condition).transpose()?,
            timeout_ms: input.timeout_ms,
            expected_schema_ref: input.expected_schema_ref,
            branch: input.branch.map(BranchId::from),
            depends_on_mode: decode_dependency_mode(input.depends_on_mode),
            allowed_tools: input.allowed_tools,
            blocked_tools: input.blocked_tools,
            output_format: decode_step_output_format(input.output_format),
        },
    ))
}

fn decode_dispatch_mode(input: MobDispatchModeInput) -> DispatchMode {
    match input {
        MobDispatchModeInput::FanOut => DispatchMode::FanOut,
        MobDispatchModeInput::OneToOne => DispatchMode::OneToOne,
        MobDispatchModeInput::FanIn => DispatchMode::FanIn,
    }
}

fn decode_collection_policy(input: MobCollectionPolicyInput) -> CollectionPolicy {
    match input {
        MobCollectionPolicyInput::All => CollectionPolicy::All,
        MobCollectionPolicyInput::Any => CollectionPolicy::Any,
        MobCollectionPolicyInput::Quorum { n } => CollectionPolicy::Quorum { n },
    }
}

fn decode_dependency_mode(input: MobDependencyModeInput) -> DependencyMode {
    match input {
        MobDependencyModeInput::All => DependencyMode::All,
        MobDependencyModeInput::Any => DependencyMode::Any,
    }
}

fn decode_step_output_format(input: MobStepOutputFormatInput) -> StepOutputFormat {
    match input {
        MobStepOutputFormatInput::Json => StepOutputFormat::Json,
        MobStepOutputFormatInput::Text => StepOutputFormat::Text,
    }
}

fn decode_condition(input: MobConditionExprInput) -> Result<ConditionExpr, String> {
    Ok(match input {
        MobConditionExprInput::Eq { path, value } => ConditionExpr::Eq { path, value },
        MobConditionExprInput::In { path, values } => ConditionExpr::In { path, values },
        MobConditionExprInput::Gt { path, value } => ConditionExpr::Gt { path, value },
        MobConditionExprInput::Lt { path, value } => ConditionExpr::Lt { path, value },
        MobConditionExprInput::And { exprs } => ConditionExpr::And {
            exprs: exprs
                .into_iter()
                .map(decode_condition)
                .collect::<Result<Vec<_>, _>>()?,
        },
        MobConditionExprInput::Or { exprs } => ConditionExpr::Or {
            exprs: exprs
                .into_iter()
                .map(decode_condition)
                .collect::<Result<Vec<_>, _>>()?,
        },
        MobConditionExprInput::Not { expr } => ConditionExpr::Not {
            expr: Box::new(decode_condition(*expr)?),
        },
    })
}

fn decode_frame_spec(input: MobFrameSpecInput) -> Result<FrameSpec, String> {
    Ok(FrameSpec {
        nodes: input
            .nodes
            .into_iter()
            .map(|(node_id, node)| decode_flow_node(node_id, node))
            .collect::<Result<IndexMap<_, _>, _>>()?,
    })
}

fn decode_flow_node(
    node_id: String,
    input: MobFlowNodeInput,
) -> Result<(FlowNodeId, FlowNodeSpec), String> {
    Ok((
        FlowNodeId::from(node_id),
        match input {
            MobFlowNodeInput::Step(step) => FlowNodeSpec::Step(decode_frame_step(step)),
            MobFlowNodeInput::RepeatUntil(loop_spec) => {
                FlowNodeSpec::RepeatUntil(decode_repeat_until(loop_spec)?)
            }
        },
    ))
}

fn decode_frame_step(input: MobFrameStepInput) -> FrameStepSpec {
    FrameStepSpec {
        step_id: StepId::from(input.step_id),
        depends_on: input.depends_on.into_iter().map(FlowNodeId::from).collect(),
        depends_on_mode: decode_dependency_mode(input.depends_on_mode),
        branch: input.branch.map(BranchId::from),
    }
}

fn decode_repeat_until(input: MobRepeatUntilInput) -> Result<RepeatUntilSpec, String> {
    Ok(RepeatUntilSpec {
        loop_id: LoopId::from(input.loop_id),
        depends_on: input.depends_on.into_iter().map(FlowNodeId::from).collect(),
        depends_on_mode: decode_dependency_mode(input.depends_on_mode),
        body: decode_frame_spec(input.body)?,
        until: decode_condition(input.until)?,
        max_iterations: input.max_iterations,
    })
}

fn decode_topology(input: MobTopologySpecInput) -> TopologySpec {
    TopologySpec {
        mode: match input.mode {
            MobPolicyModeInput::Advisory => PolicyMode::Advisory,
            MobPolicyModeInput::Strict => PolicyMode::Strict,
        },
        rules: input
            .rules
            .into_iter()
            .map(|rule| TopologyRule {
                from_role: ProfileName::from(rule.from_role),
                to_role: ProfileName::from(rule.to_role),
                allowed: rule.allowed,
            })
            .collect(),
    }
}

fn decode_limits(input: MobLimitsSpecInput) -> LimitsSpec {
    LimitsSpec {
        max_flow_duration_ms: input.max_flow_duration_ms,
        max_step_retries: input.max_step_retries,
        max_orphaned_turns: input.max_orphaned_turns,
        cancel_grace_timeout_ms: input.cancel_grace_timeout_ms,
        max_active_nodes: input.max_active_nodes,
        max_active_frames: input.max_active_frames,
        max_frame_depth: input.max_frame_depth,
    }
}

fn decode_spawn_policy(input: MobSpawnPolicyInput) -> SpawnPolicyConfig {
    match input {
        MobSpawnPolicyInput::None => SpawnPolicyConfig::None,
        MobSpawnPolicyInput::Auto { profile_map } => SpawnPolicyConfig::Auto {
            profile_map: profile_map
                .into_iter()
                .map(|(target, profile_name)| (target, ProfileName::from(profile_name)))
                .collect(),
        },
    }
}

fn decode_event_router(input: MobEventRouterConfigInput) -> EventRouterConfig {
    EventRouterConfig {
        buffer_size: input.buffer_size,
        include_patterns: input.include_patterns,
        exclude_patterns: input.exclude_patterns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_contracts::{
        WireContentInput,
        wire::{
            MobConditionExprInput, MobDispatchModeInput, MobFlowNodeInput, MobFlowSpecInput,
            MobFlowStepInput, MobFrameSpecInput, MobFrameStepInput, MobProfileInput,
            MobRepeatUntilInput, MobToolConfigInput,
        },
    };
    use std::collections::BTreeMap;

    #[test]
    fn decode_public_mob_definition_maps_nested_flow_shapes() {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "lead".to_string(),
            MobProfileInput {
                model: "gpt-5.4".to_string(),
                skills: vec!["triage".to_string()],
                tools: MobToolConfigInput {
                    comms: true,
                    mob: true,
                    ..MobToolConfigInput::default()
                },
                peer_description: "lead".to_string(),
                external_addressable: false,
                backend: Some(WireMobBackendKind::External),
                runtime_mode: WireMobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: Some(2),
                output_schema: Some(serde_json::json!({"type":"object"})),
                provider_params: Some(serde_json::json!({"reasoning_effort":"medium"})),
            },
        );

        let mut steps = BTreeMap::new();
        steps.insert(
            "plan".to_string(),
            MobFlowStepInput {
                role: "lead".to_string(),
                message: WireContentInput::Text("Plan work".to_string()),
                depends_on: Vec::new(),
                dispatch_mode: MobDispatchModeInput::OneToOne,
                collection_policy: MobCollectionPolicyInput::All,
                condition: None,
                timeout_ms: Some(5_000),
                expected_schema_ref: Some("#/$defs/plan".to_string()),
                branch: Some("winner".to_string()),
                depends_on_mode: MobDependencyModeInput::All,
                allowed_tools: Some(vec!["comms/send".to_string()]),
                blocked_tools: Some(vec!["mob_create".to_string()]),
                output_format: MobStepOutputFormatInput::Text,
            },
        );

        let mut nodes = BTreeMap::new();
        nodes.insert(
            "step-1".to_string(),
            MobFlowNodeInput::Step(MobFrameStepInput {
                step_id: "plan".to_string(),
                depends_on: Vec::new(),
                depends_on_mode: MobDependencyModeInput::All,
                branch: Some("winner".to_string()),
            }),
        );
        nodes.insert(
            "loop".to_string(),
            MobFlowNodeInput::RepeatUntil(MobRepeatUntilInput {
                loop_id: "loop-1".to_string(),
                depends_on: vec!["step-1".to_string()],
                depends_on_mode: MobDependencyModeInput::Any,
                body: MobFrameSpecInput {
                    nodes: BTreeMap::from([(
                        "inner".to_string(),
                        MobFlowNodeInput::Step(MobFrameStepInput {
                            step_id: "plan".to_string(),
                            depends_on: Vec::new(),
                            depends_on_mode: MobDependencyModeInput::All,
                            branch: None,
                        }),
                    )]),
                },
                until: MobConditionExprInput::Eq {
                    path: "$.done".to_string(),
                    value: serde_json::json!(true),
                },
                max_iterations: 3,
            }),
        );

        let definition = decode_public_mob_definition(MobDefinitionInput {
            id: "triage".to_string(),
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: Default::default(),
            skills: BTreeMap::new(),
            backend: MobBackendConfigInput::default(),
            flows: BTreeMap::from([(
                "main".to_string(),
                MobFlowSpecInput {
                    description: Some("main flow".to_string()),
                    steps,
                    root: Some(MobFrameSpecInput { nodes }),
                },
            )]),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
        })
        .expect("decode typed mob definition");

        let lead = definition
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile");
        assert_eq!(lead.backend, Some(MobBackendKind::External));
        assert_eq!(lead.runtime_mode, MobRuntimeMode::TurnDriven);
        assert_eq!(lead.tools.rust_bundles, Vec::<String>::new());

        let flow = definition
            .flows
            .get(&FlowId::from("main"))
            .expect("main flow");
        assert_eq!(flow.description.as_deref(), Some("main flow"));
        assert!(flow.root.is_some());
        let step = flow.steps.get(&StepId::from("plan")).expect("plan step");
        assert_eq!(step.branch, Some(BranchId::from("winner")));
        assert_eq!(step.output_format, StepOutputFormat::Text);
    }
}
