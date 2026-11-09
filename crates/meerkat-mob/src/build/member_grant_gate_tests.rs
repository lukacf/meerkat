//! The member grant gate, as a member build lowers it from its own profile
//! and its role's current profile, its placement refusal and its resume.
//! Every definition here is read from TOML or JSON, as an author writes it.

use super::*;
use meerkat_core::ToolCategoryOverrides;

/// A mob `test-mob` whose `lead` role declares `tools` beside `comms`.
fn definition(tools: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"test-mob\"\n\n[profiles.lead]\nmodel = \"claude-sonnet-4-5\"\n\n\
         [profiles.lead.tools]\ncomms = true\n{tools}\n"
    ))
    .expect("the definition parses")
}

fn lead(definition: &MobDefinition) -> &Profile {
    definition.profiles[&ProfileName::from("lead")]
        .as_inline()
        .expect("an inline profile")
}

/// The role and the member of every build here.
struct Member {
    role: ProfileName,
    identity: AgentIdentity,
}

impl Member {
    fn lead() -> Self {
        Self {
            role: ProfileName::from("lead"),
            identity: AgentIdentity::from("lead-1"),
        }
    }

    fn params<'a>(
        &'a self,
        definition: &'a MobDefinition,
        profile: &'a Profile,
    ) -> BuildAgentConfigParams<'a> {
        BuildAgentConfigParams {
            mob_id: &definition.id,
            profile_name: &self.role,
            agent_identity: &self.identity,
            profile,
            definition,
            realm_profile_store: None,
            external_tools: None,
            compaction_curator_override: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            mob_incarnation: Some(crate::tests::fresh_live_incarnation()),
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        }
    }
}

/// The gate a build names, as JSON: `null` when it names none.
fn named_gate(config: &AgentBuildConfig) -> serde_json::Value {
    serde_json::to_value(&config.grant_gated_tools).expect("the gate serializes")
}

/// Every member build names its gate (the empty gate when its
/// profiles gate nothing, so a member resume never restores an older
/// recorded gate), and a gated family whose switch is off is mounted, closed
/// by the gate, never disabled.
#[tokio::test]
async fn a_member_build_names_its_gate_and_mounts_a_gated_family() {
    let member = Member::lead();
    let gated = definition("grant_gated = { families = [\"shell\"] }");
    let config = build_agent_config(member.params(&gated, lead(&gated)))
        .await
        .expect("a gated member build lowers");
    assert_eq!(
        named_gate(&config),
        serde_json::json!({"families": ["shell"]}),
        "the member build names its profile's gate"
    );
    assert_eq!(
        config.override_shell,
        ToolCategoryOverride::Enable,
        "the gated family is mounted (the gate keeps it closed), not disabled"
    );

    let plain = definition("");
    let config = build_agent_config(member.params(&plain, lead(&plain)))
        .await
        .expect("an ungated member build lowers");
    assert_eq!(
        named_gate(&config),
        serde_json::json!({}),
        "an ungated member build names the empty gate"
    );
    assert_eq!(config.override_shell, ToolCategoryOverride::Disable);
}

/// A spawn-time category override that disables a gated family also
/// removes it from the member's own gate, so an explicit disable still means
/// no such tools.
#[tokio::test]
async fn a_spawn_time_disable_drops_the_family_from_the_member_gate() {
    let gated = definition("shell = true\ngrant_gated = { families = [\"shell\"] }");
    let mut profile = lead(&gated).clone();
    crate::runtime::spec_compiler::apply_tool_category_overrides(
        &mut profile,
        ToolCategoryOverrides {
            shell: ToolCategoryOverride::Disable,
            ..ToolCategoryOverrides::default()
        },
    );
    let member = Member::lead();
    let config = build_agent_config(member.params(&gated, &profile))
        .await
        .expect("the member build lowers");
    assert_eq!(config.override_shell, ToolCategoryOverride::Disable);
    assert_eq!(
        named_gate(&config),
        serde_json::json!({}),
        "the disabled family left the member's own gate"
    );
}

/// The resume row: a member whose session recorded a family as disabled
/// (built before its profile gated it) resumes with the family mounted
/// closed: the gate its profile declares now wins over the recorded
/// category, so the resume is not refused as a disabled gated family.
#[tokio::test]
async fn a_member_resume_mounts_its_gated_family_over_a_recorded_disable() {
    let gated = definition("grant_gated = { families = [\"shell\"] }");
    let session_id = SessionId::new();
    let mut resumed_session = Session::with_id(session_id.clone());
    resumed_session
        .set_session_metadata(SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::session_metadata_schema_version(),
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::session::SessionTooling {
                builtins: ToolCategoryOverride::Disable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Enable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Disable,
                workgraph: ToolCategoryOverride::Disable,
                image_generation: ToolCategoryOverride::Disable,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                spawn_tool_access_policy: None,
                application_tool_policy: meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
                active_skills: None,
                grant_gated: Default::default(),
            },
            keep_alive: false,
            comms_name: Some("test-mob/lead/lead-1".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "test-mob".to_string(),
                role: "lead".to_string(),
                member: "lead-1".to_string(),
            }),
        })
        .expect("session metadata");

    let member = Member::lead();
    let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
        base: member.params(&gated, lead(&gated)),
        expected_session_id: &session_id,
        resume_from_role: None,
        resumed_session,
    })
    .await
    .expect("the member resume lowers");
    assert_eq!(
        config.override_shell,
        ToolCategoryOverride::Enable,
        "the profile's gate wins over the recorded disable"
    );
    assert_eq!(
        named_gate(&config),
        serde_json::json!({"families": ["shell"]})
    );
}

/// The profile of these rows, read from JSON as a wire profile arrives.
fn json_profile(tools: serde_json::Value) -> Profile {
    serde_json::from_value(serde_json::json!({
        "model": "claude-sonnet-4-5",
        "provider": "anthropic",
        "tools": tools,
    }))
    .expect("the profile decodes")
}

/// A profile that gates anything cannot be placed on another host
/// (its portable form carries no gate), and the role's current gate rides a
/// placed member's own profile, so a member of a gated role is refused
/// placement too, never placed without the role's gate.
#[test]
fn a_gated_profile_and_a_member_of_a_gated_role_are_refused_placement() {
    let models = std::collections::BTreeMap::new();
    let project = |profile: &Profile| {
        crate::portable_profile::project_portable_profile(
            profile,
            crate::MobRuntimeMode::TurnDriven,
            &models,
            "worker-1",
            "worker",
            Vec::new(),
        )
    };

    let plain = json_profile(serde_json::json!({"comms": true}));
    assert!(project(&plain).is_ok(), "an ungated profile is portable");

    let gated = json_profile(serde_json::json!({
        "comms": true,
        "grant_gated": {"families": ["shell"]},
    }));
    let refused = project(&gated).expect_err("a gated profile is not portable");
    assert!(refused.contains("grant"), "{refused}");

    let mut own = json_profile(serde_json::json!({"comms": true}));
    let role = json_profile(serde_json::json!({
        "comms": true,
        "grant_gated": {"tools": ["deploy"]},
    }));
    conjoin_role_tool_restriction(&mut own.tools, &role.tools);
    assert!(
        project(&own).is_err(),
        "the role's gate rides the placed member's profile and refuses it"
    );
}
