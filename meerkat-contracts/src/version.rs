//! Contract versioning for Meerkat wire protocol.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Semantic version for the Meerkat wire contract.
///
/// Start at 0.1.0 — allow breaking changes during Phase 1-3 development.
/// Bump to 1.0.0 when the SDK Builder ships (Phase 5) and external
/// consumers exist. Before 1.0.0, minor bumps can be breaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContractVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ContractVersion {
    pub const CURRENT: Self = Self {
        major: 0,
        minor: 3,
        patch: 3,
    };

    /// Check compatibility: same major version (for 1.0+), or same major+minor (for 0.x).
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        if self.major == 0 && other.major == 0 {
            // Pre-1.0: minor bumps can be breaking
            self.minor == other.minor
        } else {
            self.major == other.major
        }
    }
}

impl fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Error returned when parsing a [`ContractVersion`] from a string fails.
#[derive(Debug, thiserror::Error)]
#[error("invalid contract version: {0}")]
pub struct ParseVersionError(String);

impl FromStr for ContractVersion {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseVersionError(s.to_string());
        let mut parts = s.splitn(3, '.');
        let major: u32 = parts.next().ok_or_else(err)?.parse().map_err(|_| err())?;
        let minor: u32 = parts.next().ok_or_else(err)?.parse().map_err(|_| err())?;
        let patch: u32 = parts.next().ok_or_else(err)?.parse().map_err(|_| err())?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_display() {
        let v = ContractVersion::CURRENT;
        let expected = format!("{}.{}.{}", v.major, v.minor, v.patch);
        assert_eq!(v.to_string(), expected);
    }

    #[test]
    fn test_from_str() {
        let v: ContractVersion = "1.2.3".parse().unwrap();
        assert_eq!(
            v,
            ContractVersion {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
    }

    #[test]
    fn test_from_str_invalid() {
        assert!("1.2".parse::<ContractVersion>().is_err());
        assert!("abc".parse::<ContractVersion>().is_err());
    }

    #[test]
    fn test_compatibility_pre_1_0() {
        let v010 = ContractVersion {
            major: 0,
            minor: 1,
            patch: 0,
        };
        let v011 = ContractVersion {
            major: 0,
            minor: 1,
            patch: 1,
        };
        let v020 = ContractVersion {
            major: 0,
            minor: 2,
            patch: 0,
        };

        assert!(v010.is_compatible_with(&v011)); // same minor
        assert!(!v010.is_compatible_with(&v020)); // different minor
    }

    #[test]
    fn test_compatibility_post_1_0() {
        let v100 = ContractVersion {
            major: 1,
            minor: 0,
            patch: 0,
        };
        let v110 = ContractVersion {
            major: 1,
            minor: 1,
            patch: 0,
        };
        let v200 = ContractVersion {
            major: 2,
            minor: 0,
            patch: 0,
        };

        assert!(v100.is_compatible_with(&v110)); // same major
        assert!(!v100.is_compatible_with(&v200)); // different major
    }

    #[test]
    fn test_ord() {
        let v010 = ContractVersion {
            major: 0,
            minor: 1,
            patch: 0,
        };
        let v011 = ContractVersion {
            major: 0,
            minor: 1,
            patch: 1,
        };
        let v100 = ContractVersion {
            major: 1,
            minor: 0,
            patch: 0,
        };
        assert!(v010 < v011);
        assert!(v011 < v100);
    }
}
