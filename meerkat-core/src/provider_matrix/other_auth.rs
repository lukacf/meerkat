//! Custom/other provider auth methods (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OtherAuthMethod {
    ApiKey,
    StaticBearer,
    None,
}

impl OtherAuthMethod {
    pub const ALL: &'static [Self] = &[Self::ApiKey, Self::StaticBearer, Self::None];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "static_bearer" => Some(Self::StaticBearer),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::StaticBearer => "static_bearer",
            Self::None => "none",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in OtherAuthMethod::ALL {
            let v = *v;
            assert_eq!(OtherAuthMethod::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(OtherAuthMethod::parse("unknown"), None);
    }
}
