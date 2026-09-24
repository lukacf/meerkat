//! Skill reference resolver.
//!
//! Accepts only typed `SkillKey` ingress. Slash-delimited `/<source>/<skill>`
//! strings are NOT accepted — callers whose input is a user-facing slash
//! reference must parse at their boundary (e.g. CLI parse) and construct a
//! `SkillKey` explicitly.

use meerkat_core::skills::{SkillError, SkillKey, SkillName, SourceUuid};

/// Parse a `"<source_uuid>/<skill_name>"` tuple into a `SkillKey`.
///
/// This is the single canonical parser for the CLI/user-facing slash form.
/// It is intentionally narrow: exactly one `/`, both halves typed.
pub fn parse_skill_key_pair(raw: &str) -> Result<SkillKey, SkillError> {
    let (source_part, skill_part) = raw.split_once('/').ok_or_else(|| {
        SkillError::Parse(format!("expected '<source_uuid>/<skill_name>', got '{raw}'").into())
    })?;
    if skill_part.contains('/') {
        return Err(SkillError::Parse(
            format!("expected '<source_uuid>/<skill_name>', got '{raw}'").into(),
        ));
    }
    let source_uuid = SourceUuid::parse(source_part)?;
    let skill_name = SkillName::parse(skill_part)?;
    Ok(SkillKey {
        source_uuid,
        skill_name,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_valid_pair() {
        let key =
            parse_skill_key_pair("dc256086-0d2f-4f61-a307-320d4148107f/email-extractor").unwrap();
        assert_eq!(key.skill_name.as_str(), "email-extractor");
    }

    #[test]
    fn test_rejects_missing_slash() {
        assert!(parse_skill_key_pair("email-extractor").is_err());
    }

    #[test]
    fn test_rejects_extra_slash() {
        assert!(parse_skill_key_pair("dc256086-0d2f-4f61-a307-320d4148107f/a/b").is_err());
    }

    #[test]
    fn test_rejects_invalid_uuid() {
        assert!(parse_skill_key_pair("not-a-uuid/email-extractor").is_err());
    }

    #[test]
    fn test_rejects_invalid_slug() {
        assert!(
            parse_skill_key_pair("dc256086-0d2f-4f61-a307-320d4148107f/EmailExtractor").is_err()
        );
    }
}
