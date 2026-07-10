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
async fn skills_feature_links_comms_owned_mob_communication_for_cross_surface_resume() {
    let _ = meerkat::SESSION_VERSION;
    let source = EmbeddedSkillSource::new();
    let key = builtin_skill_key("mob-communication");
    let result = source.load(&key).await;
    assert!(
        result.is_ok(),
        "the skills facade feature must mechanically link the comms-owned \
         mob-communication declaration"
    );
    let Ok(skill) = result else {
        unreachable!("asserted embedded mob communication skill is available above");
    };

    assert_eq!(
        skill.descriptor.key.skill_name.as_str(),
        "mob-communication"
    );
    assert_eq!(skill.descriptor.name, "mob-communication");
    assert_eq!(
        skill.descriptor.description,
        "How to communicate with peers in a collaborative mob"
    );
    // Content pin against dead-copy swaps: the load-bearing operating rules
    // must be present in the feature-owned body that the facade links.
    assert!(
        skill.body.contains("peer_request")
            && skill.body.contains("peer_message")
            && skill.body.contains("do not reply"),
        "the canonical body's concrete tool-kind guidance must be present in \
         the registered mob-communication skill"
    );
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
        "mob-communication",
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
    assert!(!without_caps.iter().any(|name| name == "mob-communication"));

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

    let comms_caps = embedded_skill_names_with_capabilities(&["comms"]).await;
    assert!(comms_caps.iter().any(|name| name == "mob-communication"));
}

#[cfg(not(feature = "comms"))]
#[test]
fn skills_only_linkage_keeps_comms_capability_not_compiled() {
    let config = meerkat_core::Config::default();
    let (registration, status) = resolve_capabilities(&config)
        .into_iter()
        .find(|(registration, _)| registration.id == meerkat_contracts::CapabilityId::Comms)
        .expect("the linked comms owner must declare its capability metadata");

    assert_eq!(registration.requires_feature, Some("comms"));
    assert!(matches!(
        status,
        CapabilityStatus::NotCompiled { ref feature } if feature.as_ref() == "comms"
    ));
    assert!(
        !available_capabilities(&config).contains(&meerkat_contracts::CapabilityId::Comms),
        "skills-only linkage must not advertise comms as publicly available"
    );
}

#[cfg(feature = "comms")]
#[test]
fn comms_feature_enables_comms_capability_registration() {
    let config = meerkat_core::Config::default();
    let (_, status) = resolve_capabilities(&config)
        .into_iter()
        .find(|(registration, _)| registration.id == meerkat_contracts::CapabilityId::Comms)
        .expect("comms feature must link the owner capability registration");

    assert!(matches!(status, CapabilityStatus::Available));
    assert!(available_capabilities(&config).contains(&meerkat_contracts::CapabilityId::Comms));
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
