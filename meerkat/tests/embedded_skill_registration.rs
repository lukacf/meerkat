#![cfg(feature = "skills")]

use meerkat_core::skills::{SkillId, SkillSource};
use meerkat_skills::EmbeddedSkillSource;

#[tokio::test]
async fn base_crate_registers_mob_communication_for_cross_surface_resume() {
    let _ = meerkat::SESSION_VERSION;
    let source = EmbeddedSkillSource::new();
    let result = source.load(&SkillId("mob-communication".to_string())).await;
    assert!(
        result.is_ok(),
        "mob-communication should be registered from the base crate"
    );
    let Ok(skill) = result else {
        unreachable!("asserted embedded mob communication skill is available above");
    };

    assert_eq!(skill.descriptor.id.0, "mob-communication");
    assert_eq!(skill.descriptor.name, "mob-communication");
    assert_eq!(
        skill.descriptor.description,
        "How to communicate with peers in a collaborative mob"
    );
}
