//! Self-hosted auth methods (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelfHostedAuthMethod {
    ApiKey,
    None,
    StaticBearer,
}

impl SelfHostedAuthMethod {
    pub const ALL: &'static [Self] = &[Self::ApiKey, Self::None, Self::StaticBearer];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "none" => Some(Self::None),
            "static_bearer" => Some(Self::StaticBearer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::None => "none",
            Self::StaticBearer => "static_bearer",
        }
    }

    /// The persisted credential mode this auth method stores in the
    /// `TokenStore`, or `None` for authless transports. Typed owner of the
    /// auth-method -> persisted-mode mapping (replaces the string-keyed
    /// `persisted_auth_mode_for_auth_method` decision table).
    pub fn persisted_auth_mode(self) -> Option<crate::auth::token_store::PersistedAuthMode> {
        use crate::auth::token_store::PersistedAuthMode;
        match self {
            Self::ApiKey => Some(PersistedAuthMode::ApiKey),
            Self::StaticBearer => Some(PersistedAuthMode::StaticBearer),
            Self::None => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in SelfHostedAuthMethod::ALL {
            let v = *v;
            assert_eq!(SelfHostedAuthMethod::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(SelfHostedAuthMethod::parse("unknown"), None);
    }
}
