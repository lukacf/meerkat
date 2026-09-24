//! Skill registration via `inventory`.
//!
//! Component crates register embedded skills via `inventory::submit!`.

use meerkat_core::skills::{SkillError, SkillName};

/// Self-registration entry for an embedded skill.
///
/// Uses `&'static str` for all fields to be compatible with
/// `inventory::submit!`, which requires static data. Converted to
/// `SkillDescriptor` / `SkillDocument` by the embedded skill source.
pub struct SkillRegistration {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub scope: meerkat_core::skills::SkillScope,
    pub requires_capabilities: &'static [&'static str],
    pub body: &'static str,
    pub extensions: &'static [(&'static str, &'static str)],
}

inventory::collect!(SkillRegistration);

/// Typed, fail-closed identity for an embedded `SkillRegistration`.
///
/// The full registration id IS the builtin skill identity — there is no
/// "trailing path segment" convention. Parsing rejects anything that is not a
/// valid canonical skill slug (e.g. ids containing `/`), so distinct static
/// registrations can never silently collapse to one loadable builtin key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationId(SkillName);

impl RegistrationId {
    /// Parse the full registration id as a canonical skill slug. Fails closed
    /// on any non-slug id (including slash-namespaced ids).
    pub fn parse(raw: &str) -> Result<Self, SkillError> {
        SkillName::parse(raw).map(Self).map_err(|e| {
            SkillError::Parse(format!("invalid embedded skill id '{raw}': {e}").into())
        })
    }

    /// The canonical skill name carried by this registration id.
    pub fn skill_name(&self) -> &SkillName {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Collect all inventory-registered embedded skills.
pub fn collect_registered_skills() -> Vec<&'static SkillRegistration> {
    inventory::iter::<SkillRegistration>.into_iter().collect()
}
