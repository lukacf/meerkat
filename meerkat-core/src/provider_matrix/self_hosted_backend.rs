//! Self-hosted backend kinds (typed, provider-owned).

use super::self_hosted_auth::SelfHostedAuthMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelfHostedBackendKind {
    SelfHosted,
    OpenAiCompatible,
}

impl SelfHostedBackendKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "self_hosted" => Some(Self::SelfHosted),
            "openai_compatible" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelfHosted => "self_hosted",
            Self::OpenAiCompatible => "openai_compatible",
        }
    }

    /// The self-hosted auth methods this backend supports — the provider-owned
    /// (backend, auth) compatibility policy that the provider-runtime
    /// `supports()` seam delegates to (dogma rows #122/#178). Both backend
    /// kinds accept the same minimal credential set.
    pub fn supported_auth_methods(self) -> &'static [SelfHostedAuthMethod] {
        match self {
            Self::SelfHosted | Self::OpenAiCompatible => &[
                SelfHostedAuthMethod::ApiKey,
                SelfHostedAuthMethod::None,
                SelfHostedAuthMethod::StaticBearer,
            ],
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
            SelfHostedBackendKind::SelfHosted,
            SelfHostedBackendKind::OpenAiCompatible,
        ] {
            assert_eq!(SelfHostedBackendKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(SelfHostedBackendKind::parse("unknown"), None);
    }
}
