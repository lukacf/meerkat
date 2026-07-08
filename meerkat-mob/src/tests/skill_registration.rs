#![allow(clippy::expect_used)]

use meerkat_core::skills::{SkillKey, SkillName, SkillSource};
use meerkat_skills::EmbeddedSkillSource;

fn builtin_skill_key(slug: &str) -> SkillKey {
    SkillKey::builtin(SkillName::parse(slug).expect("valid skill name"))
}

#[tokio::test]
async fn mob_crate_registers_mob_communication_skill() {
    let source = EmbeddedSkillSource::new();
    let key = builtin_skill_key("mob-communication");
    let skill = source
        .load(&key)
        .await
        .expect("mob-communication should be registered by meerkat-mob");

    assert_eq!(skill.descriptor.key, key);
    assert_eq!(skill.descriptor.name, "mob-communication");
    assert_eq!(
        skill.descriptor.description,
        "How to communicate with peers in a collaborative mob"
    );
    assert!(skill.body.contains("Mob Communication"));
}
