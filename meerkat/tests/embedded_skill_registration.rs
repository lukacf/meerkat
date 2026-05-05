#![cfg(feature = "skills")]
#![allow(clippy::expect_used)]

use meerkat_core::skills::{SkillKey, SkillName, SkillSource};
use meerkat_skills::EmbeddedSkillSource;

#[tokio::test]
async fn base_crate_registers_mob_communication_for_cross_surface_resume() {
    let _ = meerkat::SESSION_VERSION;
    let source = EmbeddedSkillSource::new();
    let key = SkillKey::builtin(SkillName::parse("mob-communication").expect("valid skill name"));
    let result = source.load(&key).await;
    assert!(
        result.is_ok(),
        "mob-communication should be registered from the base crate"
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
}

#[tokio::test]
async fn base_crate_registers_cli_reference_for_help_surface() {
    let _ = meerkat::SESSION_VERSION;
    let source = EmbeddedSkillSource::new();
    let key =
        SkillKey::builtin(SkillName::parse("meerkat-cli-reference").expect("valid skill name"));
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
