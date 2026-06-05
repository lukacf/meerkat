//! Google auth methods (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoogleAuthMethod {
    ApiKey,
    BearerApiKey,
    ExternalAuthorizer,
    Adc,
    ApiKeyExpress,
    GoogleOauth,
    ComputeAdc,
}

impl GoogleAuthMethod {
    pub const ALL: &'static [Self] = &[
        Self::ApiKey,
        Self::BearerApiKey,
        Self::ExternalAuthorizer,
        Self::Adc,
        Self::ApiKeyExpress,
        Self::GoogleOauth,
        Self::ComputeAdc,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "bearer_api_key" => Some(Self::BearerApiKey),
            "external_authorizer" => Some(Self::ExternalAuthorizer),
            "adc" => Some(Self::Adc),
            "api_key_express" => Some(Self::ApiKeyExpress),
            "google_oauth" => Some(Self::GoogleOauth),
            "compute_adc" => Some(Self::ComputeAdc),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::BearerApiKey => "bearer_api_key",
            Self::ExternalAuthorizer => "external_authorizer",
            Self::Adc => "adc",
            Self::ApiKeyExpress => "api_key_express",
            Self::GoogleOauth => "google_oauth",
            Self::ComputeAdc => "compute_adc",
        }
    }

    /// The persisted credential mode this auth method stores in the
    /// `TokenStore`, or `None` for ADC/authorizer-backed methods that hold no
    /// persisted secret. Typed owner of the auth-method -> persisted-mode
    /// mapping (replaces the string-keyed `persisted_auth_mode_for_auth_method`
    /// decision table).
    pub fn persisted_auth_mode(self) -> Option<crate::auth::token_store::PersistedAuthMode> {
        use crate::auth::token_store::PersistedAuthMode;
        match self {
            Self::ApiKey | Self::ApiKeyExpress => Some(PersistedAuthMode::ApiKey),
            Self::BearerApiKey => Some(PersistedAuthMode::StaticBearer),
            Self::GoogleOauth => Some(PersistedAuthMode::GoogleOauth),
            Self::ExternalAuthorizer | Self::Adc | Self::ComputeAdc => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in GoogleAuthMethod::ALL {
            let v = *v;
            assert_eq!(GoogleAuthMethod::parse(v.as_str()), Some(v));
        }
    }
}
