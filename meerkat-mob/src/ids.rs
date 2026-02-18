//! Newtype identifiers for mob entities.
//!
//! These types wrap `String` for compile-time safety: you cannot accidentally
//! pass a `MobId` where a `MeerkatId` is expected. They intentionally do NOT
//! implement `Deref<Target = str>` -- use `as_str()` for explicit conversion.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Unique identifier for a mob instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MobId(String);

impl MobId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for MobId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MobId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl Borrow<str> for MobId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for MobId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Unique identifier for a meerkat (agent instance) within a mob.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeerkatId(String);

impl MeerkatId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MeerkatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for MeerkatId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MeerkatId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl Borrow<str> for MeerkatId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for MeerkatId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Profile name within a mob definition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for ProfileName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ProfileName {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl Borrow<str> for ProfileName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ProfileName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mob_id_serde_roundtrip() {
        let id = MobId::from("test-mob");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"test-mob\"");
        let parsed: MobId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_meerkat_id_serde_roundtrip() {
        let id = MeerkatId::from("agent-1");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"agent-1\"");
        let parsed: MeerkatId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_profile_name_serde_roundtrip() {
        let name = ProfileName::from("orchestrator");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"orchestrator\"");
        let parsed: ProfileName = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn test_id_as_str() {
        let mob_id = MobId::from("my-mob");
        assert_eq!(mob_id.as_str(), "my-mob");

        let meerkat_id = MeerkatId::from("agent-x");
        assert_eq!(meerkat_id.as_str(), "agent-x");

        let profile = ProfileName::from("worker");
        assert_eq!(profile.as_str(), "worker");
    }

    #[test]
    fn test_id_display() {
        let id = MobId::from("display-test");
        assert_eq!(format!("{id}"), "display-test");
    }

    #[test]
    fn test_id_borrow_str() {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<MobId, u32> = BTreeMap::new();
        map.insert(MobId::from("key"), 42);
        // Borrow<str> allows lookup by &str
        assert_eq!(map.get("key"), Some(&42));
    }

    #[test]
    fn test_id_from_string() {
        let owned = String::from("from-string");
        let id = MobId::from(owned);
        assert_eq!(id.as_str(), "from-string");
    }

    #[test]
    fn test_id_ordering() {
        let a = MeerkatId::from("alpha");
        let b = MeerkatId::from("beta");
        assert!(a < b);
    }
}
