//! Canonical session locator grammar shared across surfaces.

use meerkat_core::SessionId;

/// Parsed session locator input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocator {
    pub realm_id: Option<String>,
    pub session_id: SessionId,
}

/// Errors parsing or validating session locators.
#[derive(Debug, thiserror::Error)]
pub enum SessionLocatorError {
    #[error("invalid session locator: expected <session_id> or <realm_id>:<session_id>")]
    InvalidFormat,
    #[error("invalid realm id in session locator: {0}")]
    InvalidRealmId(String),
    #[error("invalid session id in session locator: {0}")]
    InvalidSessionId(String),
    #[error("session locator realm '{provided}' does not match active realm '{active}'")]
    RealmMismatch { provided: String, active: String },
}

pub fn format_session_ref(realm_id: &str, session_id: &SessionId) -> String {
    format!("{realm_id}:{session_id}")
}

impl SessionLocator {
    /// Parse either a bare `<session_id>` or `<realm_id>:<session_id>`.
    pub fn parse(input: &str) -> Result<Self, SessionLocatorError> {
        if let Some((realm_part, session_part)) = input.split_once(':') {
            if realm_part.is_empty() || session_part.is_empty() {
                return Err(SessionLocatorError::InvalidFormat);
            }
            meerkat_core::runtime_bootstrap::validate_explicit_realm_id(realm_part)
                .map_err(|_| SessionLocatorError::InvalidRealmId(realm_part.to_string()))?;
            let session_id = SessionId::parse(session_part)
                .map_err(|_| SessionLocatorError::InvalidSessionId(session_part.to_string()))?;
            return Ok(Self {
                realm_id: Some(realm_part.to_string()),
                session_id,
            });
        }
        let session_id = SessionId::parse(input)
            .map_err(|_| SessionLocatorError::InvalidSessionId(input.to_string()))?;
        Ok(Self {
            realm_id: None,
            session_id,
        })
    }

    /// Resolve a locator against an active realm id, ensuring any explicit
    /// locator realm matches.
    pub fn resolve_for_realm(
        input: &str,
        active_realm_id: &str,
    ) -> Result<SessionId, SessionLocatorError> {
        let locator = Self::parse(input)?;
        if let Some(provided) = locator.realm_id {
            if provided != active_realm_id {
                return Err(SessionLocatorError::RealmMismatch {
                    provided,
                    active: active_realm_id.to_string(),
                });
            }
        }
        Ok(locator.session_id)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn session_locator_parsing_is_unambiguous() {
        let sid = SessionId::new();
        let bare = SessionLocator::parse(&sid.to_string()).expect("bare locator should parse");
        assert!(bare.realm_id.is_none());
        assert_eq!(bare.session_id, sid);

        let explicit = SessionLocator::parse(&format!("team-alpha:{sid}"))
            .expect("session_ref locator should parse");
        assert_eq!(explicit.realm_id.as_deref(), Some("team-alpha"));
        assert_eq!(explicit.session_id, sid);
    }

    #[test]
    fn session_locator_rejects_invalid_shapes() {
        assert!(matches!(
            SessionLocator::parse("team-alpha:"),
            Err(SessionLocatorError::InvalidFormat)
        ));
        assert!(matches!(
            SessionLocator::parse("not-a-session"),
            Err(SessionLocatorError::InvalidSessionId(_))
        ));
    }

    #[test]
    fn session_locator_realm_mismatch_is_rejected() {
        let sid = SessionId::new();
        let result = SessionLocator::resolve_for_realm(&format!("alpha:{sid}"), "beta");
        assert!(matches!(
            result,
            Err(SessionLocatorError::RealmMismatch { .. })
        ));
    }
}
