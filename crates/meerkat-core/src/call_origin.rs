//! Canonical wire projection of runtime-admitted call origin.
//!
//! This initial codec represents unavailable origin only. It makes no claim
//! about a requester, represented subject, or executing agent. Native runtime
//! admission will extend this same codec; transport adapters must not invent
//! origin from a session id, caller arguments, or ambient turn metadata.

use serde::{Deserialize, Serialize};

/// Native-owned MCP metadata key. A host context provider cannot replace it.
pub const CALL_ORIGIN_META_KEY: &str = "io.meerkat/origin";

/// Transport projection, never independent evidence of permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireCallOrigin {
    /// No runtime-admitted requester/subject origin is available.
    Unavailable,
}

impl<'de> Deserialize<'de> for WireCallOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Internally tagged unit variants ignore extra fields in serde. The
        // empty struct shape enforces the strict wire contract instead.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum StrictOrigin {
            Unavailable {},
        }
        match StrictOrigin::deserialize(deserializer)? {
            StrictOrigin::Unavailable {} => Ok(Self::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_has_one_exact_wire_form() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_string(&WireCallOrigin::Unavailable)?,
            r#"{"kind":"unavailable"}"#
        );
        assert_eq!(
            serde_json::from_str::<WireCallOrigin>(r#"{"kind":"unavailable"}"#)?,
            WireCallOrigin::Unavailable
        );
        for invalid in [
            r#"{}"#,
            r#"{"kind":"admitted"}"#,
            r#"{"kind":"unavailable","requester":"claimed"}"#,
        ] {
            assert!(serde_json::from_str::<WireCallOrigin>(invalid).is_err());
        }
        Ok(())
    }
}
