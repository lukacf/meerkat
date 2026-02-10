//! Skill reference resolver.
//!
//! Resolves explicit skill references by:
//! 1. `/skill-id` syntax (slash prefix).
//! 2. Exact name match (case-insensitive).

use meerkat_core::skills::{SkillDescriptor, SkillError, SkillId};

/// Resolve a skill reference against available descriptors.
///
/// Returns the matching `SkillId` or an error if not found / ambiguous.
pub fn resolve_reference(
    reference: &str,
    available: &[SkillDescriptor],
) -> Result<SkillId, SkillError> {
    // 1. Slash-prefix ID match
    if let Some(id_str) = reference.strip_prefix('/') {
        let matches: Vec<&SkillDescriptor> = available
            .iter()
            .filter(|d| d.id.0 == id_str)
            .collect();
        return match matches.len() {
            0 => Err(SkillError::NotFound {
                id: SkillId(id_str.to_string()),
            }),
            1 => Ok(matches[0].id.clone()),
            _ => Err(SkillError::Ambiguous {
                reference: reference.to_string(),
                matches: matches.iter().map(|d| d.id.clone()).collect(),
            }),
        };
    }

    // 2. Exact name match (case-insensitive)
    let reference_lower = reference.to_lowercase();
    let matches: Vec<&SkillDescriptor> = available
        .iter()
        .filter(|d| d.name.to_lowercase() == reference_lower)
        .collect();

    match matches.len() {
        0 => Err(SkillError::NotFound {
            id: SkillId(reference.to_string()),
        }),
        1 => Ok(matches[0].id.clone()),
        _ => Err(SkillError::Ambiguous {
            reference: reference.to_string(),
            matches: matches.iter().map(|d| d.id.clone()).collect(),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SkillScope;

    fn make_descriptor(id: &str, name: &str) -> SkillDescriptor {
        SkillDescriptor {
            id: SkillId(id.to_string()),
            name: name.to_string(),
            description: String::new(),
            scope: SkillScope::Builtin,
            requires_capabilities: Vec::new(),
        }
    }

    #[test]
    fn test_resolve_by_slash_id() {
        let skills = vec![make_descriptor("shell-patterns", "Shell Patterns")];
        let result = resolve_reference("/shell-patterns", &skills).unwrap();
        assert_eq!(result, SkillId("shell-patterns".to_string()));
    }

    #[test]
    fn test_resolve_by_name() {
        let skills = vec![make_descriptor("shell-patterns", "Shell Patterns")];
        let result = resolve_reference("Shell Patterns", &skills).unwrap();
        assert_eq!(result, SkillId("shell-patterns".to_string()));
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let skills = vec![make_descriptor("shell-patterns", "Shell Patterns")];
        let result = resolve_reference("shell patterns", &skills).unwrap();
        assert_eq!(result, SkillId("shell-patterns".to_string()));
    }

    #[test]
    fn test_resolve_not_found() {
        let skills = vec![make_descriptor("shell-patterns", "Shell Patterns")];
        let result = resolve_reference("/nonexistent", &skills);
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[test]
    fn test_resolve_ambiguous() {
        let skills = vec![
            make_descriptor("a", "Same Name"),
            make_descriptor("b", "Same Name"),
        ];
        let result = resolve_reference("Same Name", &skills);
        assert!(matches!(result, Err(SkillError::Ambiguous { .. })));
    }
}
