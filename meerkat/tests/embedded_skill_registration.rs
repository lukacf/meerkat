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
