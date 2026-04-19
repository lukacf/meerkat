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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in [
            GoogleAuthMethod::ApiKey,
            GoogleAuthMethod::BearerApiKey,
            GoogleAuthMethod::ExternalAuthorizer,
            GoogleAuthMethod::Adc,
            GoogleAuthMethod::ApiKeyExpress,
            GoogleAuthMethod::GoogleOauth,
            GoogleAuthMethod::ComputeAdc,
        ] {
            assert_eq!(GoogleAuthMethod::parse(v.as_str()), Some(v));
        }
    }
}
