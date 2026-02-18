use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use crate::definition::{MobDefinition, SkillSource};
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::profile::Profile;
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::skills::SkillId;
use meerkat_core::PeerMeta;

pub struct MobAgentSessionConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub build: SessionBuildOptions,
}

fn compose_prompt(
    profile_name: &ProfileName,
    profile: &Profile,
    skills: &[(&String, &str)],
) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!("Profile: {}\n", profile_name.as_str()));
    if !profile.peer_description.is_empty() {
        prompt.push_str(&format!("Role: {}\n", profile.peer_description));
    }
    if !skills.is_empty() {
        prompt.push_str("Skills:\n");
        for (name, content) in skills {
            prompt.push_str(&format!("\n### {name}\n{content}\n"));
        }
    }
    prompt
}

pub fn build_agent_config(
    mob_id: &MobId,
    profile_name: &ProfileName,
    profile: &Profile,
    key: &MeerkatId,
    skills_content: &BTreeMap<String, String>,
    mob_comms_instructions: &str,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> MobAgentSessionConfig {
    let mut config = MobAgentSessionConfig {
        model: profile.model.clone(),
        system_prompt: None,
        max_tokens: None,
        build: SessionBuildOptions {
            override_builtins: None,
            override_shell: None,
            override_subagents: None,
            override_memory: None,
            preload_skills: None,
            ..SessionBuildOptions::default()
        },
    };

    let skill_bodies = profile
        .skills
        .iter()
        .filter_map(|skill| {
            skills_content
                .get(skill)
                .map(|content| (skill, content.as_str()))
        })
        .collect::<Vec<_>>();
    let skill_refs = profile
        .skills
        .iter()
        .map(|name| (name, SkillId(name.clone())))
        .collect::<Vec<_>>();
    let mut preload_skills = Vec::new();
    for (name, skill_id) in skill_refs {
        if skills_content.contains_key(name) {
            preload_skills.push(skill_id);
        }
    }

    let peer_meta = PeerMeta::default()
        .with_description(profile.peer_description.clone())
        .with_label("mob_id".to_string(), mob_id.as_str().to_string())
        .with_label("role".to_string(), profile_name.as_str().to_string())
        .with_label("meerkat_id".to_string(), key.as_str().to_string());

    let mut base_prompt = compose_prompt(
        profile_name,
        profile,
        &skill_bodies
            .iter()
            .map(|(name, body)| (*name, *body as &str))
            .collect::<Vec<_>>(),
    );
    if !mob_comms_instructions.is_empty() {
        base_prompt.push_str("\n\nMob comms instructions:\n");
        base_prompt.push_str(mob_comms_instructions);
    }

    config.system_prompt = Some(base_prompt);
    config.build.comms_name = Some(format!(
        "{}/{}/{}",
        mob_id.as_str(),
        profile_name.as_str(),
        key.as_str()
    ));
    config.build.peer_meta = Some(peer_meta);
    config.build.realm_id = Some(format!("mob:{}", mob_id.as_str()));
    config.build.external_tools = external_tools;
    config.build.override_builtins = Some(profile.tools.builtins);
    config.build.override_shell = Some(profile.tools.shell);
    config.build.override_subagents = Some(profile.tools.comms);
    config.build.override_memory = Some(profile.tools.memory);
    config.build.preload_skills = if preload_skills.is_empty() {
        None
    } else {
        Some(preload_skills)
    };

    config
}

pub fn to_create_session_request(
    mut config: MobAgentSessionConfig,
    prompt: String,
) -> CreateSessionRequest {
    let skill_references = config.build.preload_skills.take();

    CreateSessionRequest {
        model: config.model,
        prompt,
        system_prompt: config.system_prompt,
        max_tokens: config.max_tokens,
        event_tx: None,
        host_mode: true,
        skill_references,
        build: Some(config.build),
    }
}

pub fn hydrate_skills_content(
    definition: &MobDefinition,
) -> Result<BTreeMap<String, String>, std::io::Error> {
    let mut skills = BTreeMap::new();
    for (name, source) in &definition.skills {
        match source {
            SkillSource::Inline(body) => {
                skills.insert(name.clone(), body.clone());
            }
            SkillSource::File { file } => {
                skills.insert(name.clone(), fs::read_to_string(file)?);
            }
        }
    }
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::profile::ToolConfig;
    use meerkat_core::error::ToolError;
    use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};

    struct DummyDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for DummyDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Vec::<Arc<ToolDef>>::new().into()
        }

        async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            Err(ToolError::Unavailable {
                name: "dummy".to_string(),
                reason: "test-only".to_string(),
            })
        }
    }

    #[test]
    fn build_agent_config_sets_comms_name_and_external_tools() {
        let profile = Profile {
            model: "gpt-4o-mini".to_string(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: "worker".to_string(),
            external_addressable: true,
        };
        let dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(DummyDispatcher);
        let cfg = build_agent_config(
            &MobId::from("mob-a"),
            &ProfileName::from("dev"),
            &profile,
            &MeerkatId::from("wk-1"),
            &BTreeMap::new(),
            "comms",
            Some(dispatcher),
        );

        assert_eq!(cfg.build.comms_name.as_deref(), Some("mob-a/dev/wk-1"));
        assert!(cfg.build.external_tools.is_some());
    }
}
