//! Google backend kinds (typed, provider-owned).

use super::google_auth::GoogleAuthMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoogleBackendKind {
    GoogleGenAi,
    VertexAi,
    GoogleCodeAssist,
}

pub const GOOGLE_CODE_ASSIST_DEFAULT_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

impl GoogleBackendKind {
    pub const ALL: &'static [Self] = &[Self::GoogleGenAi, Self::VertexAi, Self::GoogleCodeAssist];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "google_genai" => Some(Self::GoogleGenAi),
            "vertex_ai" => Some(Self::VertexAi),
            "google_code_assist" => Some(Self::GoogleCodeAssist),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoogleGenAi => "google_genai",
            Self::VertexAi => "vertex_ai",
            Self::GoogleCodeAssist => "google_code_assist",
        }
    }

    /// The Google auth methods this backend supports — the provider-owned
    /// (backend, auth) compatibility policy that the provider-runtime
    /// `supports()` seam delegates to (dogma rows #122/#178).
    pub fn supported_auth_methods(self) -> &'static [GoogleAuthMethod] {
        match self {
            Self::GoogleGenAi => &[
                GoogleAuthMethod::ApiKey,
                GoogleAuthMethod::BearerApiKey,
                GoogleAuthMethod::ExternalAuthorizer,
            ],
            Self::VertexAi => &[
                GoogleAuthMethod::Adc,
                GoogleAuthMethod::ApiKeyExpress,
                GoogleAuthMethod::ExternalAuthorizer,
            ],
            Self::GoogleCodeAssist => &[
                GoogleAuthMethod::GoogleOauth,
                GoogleAuthMethod::ComputeAdc,
                GoogleAuthMethod::ExternalAuthorizer,
            ],
        }
    }
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::GoogleGenAi => "https://generativelanguage.googleapis.com",
            // Vertex has region-dependent URLs — Phase 2 leaves it empty;
            // Phase 3 populates per-region overlay.
            Self::VertexAi => "",
            Self::GoogleCodeAssist => GOOGLE_CODE_ASSIST_DEFAULT_BASE_URL,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in GoogleBackendKind::ALL {
            let v = *v;
            assert_eq!(GoogleBackendKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn google_code_assist_default_base_url_is_cloudcode() {
        assert_eq!(
            GoogleBackendKind::GoogleCodeAssist.default_base_url(),
            "https://cloudcode-pa.googleapis.com"
        );
    }
}
