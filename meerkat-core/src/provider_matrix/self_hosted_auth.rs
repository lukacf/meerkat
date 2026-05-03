//! Self-hosted auth methods (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelfHostedAuthMethod {
    ApiKey,
    None,
    StaticBearer,
}

impl SelfHostedAuthMethod {
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in [
            SelfHostedAuthMethod::ApiKey,
            SelfHostedAuthMethod::None,
            SelfHostedAuthMethod::StaticBearer,
        ] {
            assert_eq!(SelfHostedAuthMethod::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(SelfHostedAuthMethod::parse("unknown"), None);
    }
}
