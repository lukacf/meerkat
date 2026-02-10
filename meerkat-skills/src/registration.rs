//! Skill registration via `inventory`.
//!
//! Component crates register embedded skills via `inventory::submit!`.

use meerkat_core::skills::SkillScope;

/// Self-registration entry for an embedded skill.
///
/// Uses `&'static str` for all fields to be compatible with `inventory::submit!`
/// which requires static data. Converted to `SkillDescriptor` / `SkillDocument`
/// at runtime.
pub struct SkillRegistration {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub scope: SkillScope,
    pub requires_capabilities: &'static [&'static str],
    pub body: &'static str,
    pub extensions: &'static [(&'static str, &'static str)],
}

inventory::collect!(SkillRegistration);

/// Collect all registered skills.
pub fn collect_registered_skills() -> Vec<&'static SkillRegistration> {
    inventory::iter::<SkillRegistration>.into_iter().collect()
}
