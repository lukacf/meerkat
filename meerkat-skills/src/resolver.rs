//! Skill reference resolver.
//!
//! Resolves skill references by stripping the leading `/` prefix.
//! The remainder is the canonical `SkillId`. No name matching.

use meerkat_core::skills::{SkillError, SkillId};

/// Resolve a `/skill-ref` string to a `SkillId`.
///
/// Strips the leading `/` and returns the remainder as a `SkillId`.
/// Returns `NotFound` if the reference doesn't start with `/`.
pub fn resolve_reference(reference: &str) -> Result<SkillId, SkillError> {
    match reference.strip_prefix('/') {
        Some(id_str) if !id_str.is_empty() => Ok(SkillId(id_str.to_string())),
        _ => Err(SkillError::NotFound {
            id: SkillId(reference.to_string()),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_slash_namespaced_id() {
        let result = resolve_reference("/extraction/email").unwrap();
        assert_eq!(result, SkillId("extraction/email".to_string()));
    }

    #[test]
    fn test_resolve_slash_root_level() {
        let result = resolve_reference("/pdf-processing").unwrap();
        assert_eq!(result, SkillId("pdf-processing".to_string()));
    }

    #[test]
    fn test_resolve_deep_nested() {
        let result = resolve_reference("/a/b/c").unwrap();
        assert_eq!(result, SkillId("a/b/c".to_string()));
    }

    #[test]
    fn test_resolve_no_prefix_fails() {
        let result = resolve_reference("bare-name");
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[test]
    fn test_resolve_empty_after_slash_fails() {
        let result = resolve_reference("/");
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }
}
