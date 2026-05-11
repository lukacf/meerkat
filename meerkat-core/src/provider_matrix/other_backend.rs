//! Custom/other provider backend kinds (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OtherBackendKind {
    OtherApi,
}

impl OtherBackendKind {
    pub const ALL: &'static [Self] = &[Self::OtherApi];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "other_api" => Some(Self::OtherApi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OtherApi => "other_api",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in OtherBackendKind::ALL {
            let v = *v;
            assert_eq!(OtherBackendKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(OtherBackendKind::parse("unknown"), None);
    }
}
