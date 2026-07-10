#![allow(clippy::expect_used)]

use meerkat_core::skills::{SkillKey, SkillName, SkillSource};
use meerkat_skills::EmbeddedSkillSource;

fn builtin_skill_key(slug: &str) -> SkillKey {
    SkillKey::builtin(SkillName::parse(slug).expect("valid builtin skill name"))
}

#[tokio::test]
async fn comms_crate_owns_and_registers_both_embedded_comms_skills() {
    // Force the component crate itself into the test binary; successful loads
    // below then prove ownership rather than a facade-side duplicate.
    meerkat_comms::link_embedded_skill_registrations();
    let source = EmbeddedSkillSource::new();

    for slug in ["multi-agent-comms", "mob-communication"] {
        let skill = source
            .load(&builtin_skill_key(slug))
            .await
            .expect("comms-owned embedded skill must load");
        assert_eq!(skill.descriptor.key.skill_name.as_str(), slug);
    }

    let mob_skill = source
        .load(&builtin_skill_key("mob-communication"))
        .await
        .expect("mob-communication must be registered by meerkat-comms");
    assert!(
        mob_skill.body.contains("peer_request")
            && mob_skill.body.contains("peer_message")
            && mob_skill.body.contains("do not reply"),
        "the comms-owned body must retain concrete tool-kind guidance"
    );
}

#[cfg(feature = "capability")]
#[test]
fn direct_default_dependency_keeps_comms_capability_available() {
    let (_, status) = meerkat_capabilities::resolve_capabilities(&meerkat_core::Config::default())
        .into_iter()
        .find(|(registration, _)| registration.id == meerkat_capabilities::CapabilityId::Comms)
        .expect("direct default meerkat-comms dependency must register Comms");

    assert!(matches!(
        status,
        meerkat_capabilities::CapabilityStatus::Available
    ));
}
