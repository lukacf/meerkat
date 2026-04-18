//! Google backend kinds (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoogleBackendKind {
    GoogleGenAi,
    VertexAi,
    GoogleCodeAssist,
}

impl GoogleBackendKind {
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
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::GoogleGenAi => "https://generativelanguage.googleapis.com",
            // Vertex and Code Assist have region-dependent URLs — Phase 2
            // leaves them empty; Phase 3 populates per-region overlay.
            Self::VertexAi => "",
            Self::GoogleCodeAssist => "",
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
            GoogleBackendKind::GoogleGenAi,
            GoogleBackendKind::VertexAi,
            GoogleBackendKind::GoogleCodeAssist,
        ] {
            assert_eq!(GoogleBackendKind::parse(v.as_str()), Some(v));
        }
    }
}
