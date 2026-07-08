#![cfg(feature = "skills")]
#![allow(clippy::expect_used)]

use meerkat_contracts::{CapabilityStatus, available_capabilities, resolve_capabilities};
use meerkat_core::skills::{
    CapabilityId, SkillEngine, SkillFilter, SkillKey, SkillName, SkillSource,
};
use meerkat_skills::{DefaultSkillEngine, EmbeddedSkillSource};

fn builtin_skill_key(slug: &str) -> SkillKey {
    SkillKey::builtin(SkillName::parse(slug).expect("valid skill name"))
}

fn capability(slug: &str) -> CapabilityId {
    CapabilityId::parse(slug).expect("valid capability")
}

async fn embedded_skill_names_with_capabilities(caps: &[&str]) -> Vec<String> {
    let engine = DefaultSkillEngine::new(
        EmbeddedSkillSource::new(),
        caps.iter().map(|slug| capability(slug)).collect(),
    );
    engine
        .list_skills(&SkillFilter::default())
        .await
        .expect("list embedded skills")
        .into_iter()
        .map(|skill| skill.key.skill_name.to_string())
        .collect()
}

#[tokio::test]
async fn base_crate_registers_cli_reference_for_help_surface() {
    let _ = meerkat::SESSION_VERSION;
    let source = EmbeddedSkillSource::new();
    let key = builtin_skill_key("meerkat-cli-reference");
    let skill = source
        .load(&key)
        .await
        .expect("meerkat-cli-reference should be registered from the base crate");

    assert_eq!(skill.descriptor.key, key);
    assert_eq!(skill.descriptor.name, "meerkat-cli-reference");
    assert!(skill.body.contains("There is no `--tools all`"));
    assert!(
        skill
            .body
            .contains("rkat blob get <BLOB-ID> --output fox.png")
    );
}

#[tokio::test]
async fn companion_skills_load_from_builtin_source_by_stable_slug() {
    let _ = std::any::TypeId::of::<meerkat::WorkGraphService>();
    let _ = std::any::TypeId::of::<meerkat::ScheduleService>();
    let source = EmbeddedSkillSource::new();

    let slugs = [
        "task-workflow",
        "shell-patterns",
        "hook-authoring",
        "workgraph-workflow",
        "schedule-workflow",
        "skill-discovery-workflow",
        "builtin-utilities-workflow",
        #[cfg(feature = "mcp")]
        "mcp-server-setup",
        #[cfg(feature = "comms")]
        "multi-agent-comms",
        #[cfg(feature = "memory-store-session")]
        "memory-retrieval",
        #[cfg(feature = "session-store")]
        "session-management",
    ];

    for slug in slugs {
        let result = source.load(&builtin_skill_key(slug)).await;
        assert!(
            result.is_ok(),
            "{slug} should load from builtin source: {result:?}"
        );
        let Ok(doc) = result else {
            continue;
        };
        assert_eq!(doc.descriptor.key.skill_name.as_str(), slug);
    }
}

#[tokio::test]
async fn companion_skills_are_listed_only_when_capabilities_are_available() {
    let _ = std::any::TypeId::of::<meerkat::WorkGraphService>();
    let _ = std::any::TypeId::of::<meerkat::ScheduleService>();

    let without_caps = embedded_skill_names_with_capabilities(&[]).await;
    assert!(!without_caps.iter().any(|name| name == "workgraph-workflow"));
    assert!(!without_caps.iter().any(|name| name == "schedule-workflow"));

    let workgraph_caps = embedded_skill_names_with_capabilities(&["work_graph"]).await;
    assert!(
        workgraph_caps
            .iter()
            .any(|name| name == "workgraph-workflow")
    );
    assert!(
        !workgraph_caps
            .iter()
            .any(|name| name == "schedule-workflow")
    );

    let schedule_caps = embedded_skill_names_with_capabilities(&["schedule"]).await;
    assert!(schedule_caps.iter().any(|name| name == "schedule-workflow"));
    assert!(
        !schedule_caps
            .iter()
            .any(|name| name == "workgraph-workflow")
    );
}

#[test]
fn companion_skills_are_not_help_preloads() {
    let preload = meerkat::help::platform_preload_skill_names();
    assert!(!preload.iter().any(|name| name == "workgraph-workflow"));
    assert!(!preload.iter().any(|name| name == "schedule-workflow"));
}

#[test]
fn skills_crate_registers_skills_capability_for_composed_runtime() {
    let _ = meerkat::SESSION_VERSION;
    let mut config = meerkat_core::Config::default();

    assert!(
        available_capabilities(&config).contains(&meerkat_contracts::CapabilityId::Skills),
        "skills-enabled composed runtime should register the skills capability"
    );

    config.skills.enabled = false;
    let skills_status = resolve_capabilities(&config)
        .into_iter()
        .find_map(|(registration, status)| {
            (registration.id == meerkat_contracts::CapabilityId::Skills).then_some(status)
        })
        .expect("skills capability registration should exist");

    assert!(matches!(
        skills_status,
        CapabilityStatus::DisabledByPolicy { .. }
    ));
}
